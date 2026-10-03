//! Run-isolated stdio MCP for an already authenticated, admitted connection.
//! The host connector owns the pipe and the one-way revoker. Tool arguments
//! never supply identities, transport configuration, credentials or read views.

use std::{
    future::pending,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};
use super::{Operation, ReadError, ReadView, ScopedReader, bounded_json};

const PROTOCOL: &str = "2025-06-18";

/// Closed tool schemas for the granted subset. Export pages choose nodes or
/// edges explicitly. Page cursors bind the operation, arguments and full view.
#[must_use]
pub fn tool_definitions(view: &ReadView) -> Vec<Value> {
    view.operations.iter().map(|op| {
        let text = json!({"type":"string", "minLength":1, "maxLength":4096});
        let (description, mut props, required) = match op {
            Operation::Status => ("Pinned graph status; workspace applicability is unknown.", json!({}), vec!["repo"]),
            Operation::Search => ("Literal substring search over authorized entity names.", json!({"query":text}), vec!["repo", "query"]),
            Operation::Lookup => ("Authorized entity lookup.", json!({"id":text}), vec!["repo", "id"]),
            Operation::Neighbors => ("Paged incoming/outgoing edges with explicit endpoints.", json!({"id":text, "rel":{"type":"string", "enum":crate::all_relation_types()}}), vec!["repo", "id"]),
            Operation::Path => ("Bounded undirected path; null when absent.", json!({"from":text, "to":text,
                "max_hops":{"type":"integer", "minimum":1, "maximum":view.budgets.max_hops}}), vec!["repo", "from", "to"]),
            Operation::Explain => ("Source-linked entity metadata and scoped degree counts.", json!({"id":text}), vec!["repo", "id"]),
            Operation::Evidence => ("Complete authorized relationship evidence.", json!({"rel":text}), vec!["repo", "rel"]),
            Operation::Export => ("Page the pinned authorized graph.", json!({"kind":{"type":"string", "enum":["nodes", "edges"]}}), vec!["repo", "kind"]),
        };
        props["repo"] = json!({"type":"string", "const":view.repo});
        if matches!(op, Operation::Search | Operation::Neighbors | Operation::Export) {
            props["limit"] = json!({"type":"integer", "minimum":1, "maximum":view.budgets.max_page_size});
            props["cursor"] = json!({"type":"string"});
        }
        json!({"name":op, "description":description,
            "inputSchema":{"type":"object", "properties":props, "required":required, "additionalProperties":false},
            "annotations":{"readOnlyHint":true, "destructiveHint":false}})
    }).collect()
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> std::future::Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

struct RevokeOnDrop(super::Revoker);

impl Drop for RevokeOnDrop {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

struct Running {
    id: Value,
    deadline: tokio::time::Instant,
    task: AbortOnDrop<Value>,
}
enum Event {
    Frame(Option<Result<Vec<u8>, ()>>),
    Done(Result<Value, tokio::task::JoinError>),
    Stop,
}

async fn read_frames<I: AsyncRead + Unpin>(
    input: I,
    cap: usize,
    tx: mpsc::Sender<Result<Vec<u8>, ()>>,
) {
    let mut input = BufReader::new(input);
    loop {
        let mut line = Vec::new();
        match (&mut input)
            .take(cap as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(0) => break,
            Ok(_) if line.len() <= cap => {
                if tx.send(Ok(line)).await.is_err() {
                    break;
                }
            }
            _ => {
                let _ = tx.send(Err(())).await;
                break;
            }
        }
    }
}

/// Serve one run and one connection. EOF, expiry, revocation and output failure
/// settle the active task. MCP cancellation aborts only the matching request.
/// Input and output buffers are bounded; only one call may be in flight.
pub async fn serve<I, O>(reader: ScopedReader, input: I, mut output: O) -> std::io::Result<()>
where
    I: AsyncRead + Unpin + Send + 'static,
    O: AsyncWrite + Unpin,
{
    let reader = Arc::new(reader);
    // Also settle children if the connector drops/aborts the entire server
    // future, rather than returning through the normal EOF cleanup below.
    let _revoke_on_drop = RevokeOnDrop(reader.revoker());
    let identity = reader.view.identity.clone();
    let expires = expiry_deadline(reader.view.expires_at)?;
    let cap = reader.view.budgets.max_request_bytes + 2048;
    let (tx, mut rx) = mpsc::channel(1);
    let input_task = AbortOnDrop(tokio::spawn(read_frames(input, cap, tx)));
    let mut revoked = reader.revoker.0.subscribe();
    let mut initialized = false;
    let mut running: Option<Running> = None;
    let result = async {
        loop {
            let event = tokio::select! {
                biased;
                _ = revoked.wait_for(|r| *r) => Event::Stop,
                () = tokio::time::sleep_until(expires) => Event::Stop,
                frame = rx.recv() => Event::Frame(frame),
                done = async { match running.as_mut() { Some(call) => (&mut call.task).await, None => pending().await } } => Event::Done(done),
            };
            let (response, deadline) = match event {
                Event::Stop | Event::Frame(None | Some(Err(()))) => break,
                Event::Done(result) => {
                    let Some(call) = running.take() else { break; };
                    reader.check(&identity).map_err(std::io::Error::other)?;
                    (result.unwrap_or_else(|_| blocked(&call.id, ReadError::Backend)), Some(call.deadline))
                }
                Event::Frame(Some(Ok(line))) => {
                    if line.iter().all(u8::is_ascii_whitespace) { continue; }
                    let Ok(req) = serde_json::from_slice::<Value>(&line) else {
                        write(&mut output, &rpc_error(&Value::Null, -32700, "parse error"), &reader, expires, None).await?; continue;
                    };
                    if !req.is_object() || req["jsonrpc"] != "2.0" || !req["method"].is_string() {
                        write(&mut output, &rpc_error(&Value::Null, -32600, "invalid request"), &reader, expires, None).await?; continue;
                    }
                    let method = req["method"].as_str().unwrap_or_default();
                    if req.get("id").is_none() {
                        if method == "notifications/cancelled" && running.as_ref().is_some_and(|call| call.id == req["params"]["requestId"]) {
                            if let Some(call) = running.take() {
                                call.task.0.abort(); let _ = call.task.await;
                                write(&mut output, &rpc_error(&call.id, -32800, "request cancelled"), &reader, expires, None).await?;
                            }
                        }
                        continue;
                    }
                    let id = &req["id"];
                    if !(id.is_i64() || id.is_u64() || id.as_str().is_some_and(|s| s.len() <= 128 && !s.chars().any(char::is_control))) {
                        write(&mut output, &rpc_error(&Value::Null, -32600, "invalid request id"), &reader, expires, None).await?; continue;
                    }
                    if let Err(error) = reader.check(&identity) {
                        write(&mut output, &blocked(id, error), &reader, expires, None).await?;
                        break;
                    }
                    let response = match method {
                        "initialize" if !initialized => {
                            initialized = true;
                            json!({"jsonrpc":"2.0", "id":id, "result":{"protocolVersion":PROTOCOL,
                                "capabilities":{"tools":{"listChanged":false}},
                                "serverInfo":{"name":"chaosbox-read-view", "version":env!("CARGO_PKG_VERSION")}}})
                        }
                        "ping" => json!({"jsonrpc":"2.0", "id":id, "result":{}}),
                        _ if !initialized => rpc_error(id, -32600, "server not initialized"),
                        "tools/list" => list(id, &req["params"], &reader.view),
                        "tools/call" if running.is_none() => {
                            running = Some(call_task(&reader, id, &req["params"]));
                            continue;
                        }
                        "tools/call" => rpc_error(id, -32000, "one call is already in flight; cancel or await it"),
                        _ => rpc_error(id, -32601, "unsupported method"),
                    };
                    (response, None)
                }
            };
            write(&mut output, &response, &reader, expires, deadline).await?;
        }
        Ok(())
    }.await;
    reader.revoker.revoke();
    if let Some(call) = running {
        call.task.0.abort();
        let _ = call.task.await;
    }
    input_task.0.abort();
    let _ = input_task.await;
    result
}

fn call_task(reader: &Arc<ScopedReader>, id: &Value, params: &Value) -> Running {
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(reader.view.budgets.timeout_ms);
    let name = params["name"].as_str().unwrap_or_default().to_owned();
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let (reader, identity, task_id) =
        (Arc::clone(reader), reader.view.identity.clone(), id.clone());
    Running {
        id: id.clone(),
        deadline,
        task: AbortOnDrop(tokio::spawn(async move {
            match reader.call_until(&identity, &name, args, deadline).await {
                Ok(value) => json!({"jsonrpc":"2.0", "id":task_id,
                "result":{"content":[{"type":"text", "text":value.to_string()}]}}),
                Err(error) => blocked(&task_id, error),
            }
        })),
    }
}

fn list(id: &Value, params: &Value, view: &ReadView) -> Value {
    let defs = tool_definitions(view);
    let cursor = match params.get("cursor") {
        None => 0,
        Some(Value::String(s)) => match s.parse::<usize>() {
            Ok(n) if n < defs.len() => n,
            _ => return rpc_error(id, -32602, "invalid cursor"),
        },
        _ => return rpc_error(id, -32602, "invalid cursor"),
    };
    let mut result = json!({"tools": &defs[cursor..(cursor + 1).min(defs.len())]});
    if cursor + 1 < defs.len() {
        result["nextCursor"] = json!((cursor + 1).to_string());
    }
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message}})
}

fn blocked(id: &Value, error: ReadError) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "result":{"isError":true,
        "content":[{"type":"text", "text":json!({"blocked":true, "code":error.code(), "message":error.to_string()}).to_string()}]}})
}

async fn write<O: AsyncWrite + Unpin>(
    output: &mut O,
    response: &Value,
    reader: &ScopedReader,
    expires: tokio::time::Instant,
    request_deadline: Option<tokio::time::Instant>,
) -> std::io::Result<()> {
    let view = &reader.view;
    reader
        .check(&view.identity)
        .map_err(std::io::Error::other)?;
    let mut revoked = reader.revoker.0.subscribe();
    let deadline = request_deadline.unwrap_or_else(|| {
        tokio::time::Instant::now() + Duration::from_millis(view.budgets.timeout_ms)
    });
    let mut bytes = bounded_json(response, view.budgets.max_response_bytes).unwrap_or_else(|_| {
        blocked(&response["id"], ReadError::Budget)
            .to_string()
            .into_bytes()
    });
    bytes.push(b'\n');
    if tokio::time::Instant::now() >= expires {
        return Err(std::io::Error::other(ReadError::Expired));
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(std::io::Error::other(ReadError::Deadline));
    }
    tokio::select! {
        biased;
        _ = revoked.wait_for(|r| *r) => Err(std::io::Error::other(ReadError::Revoked)),
        () = tokio::time::sleep_until(expires) => Err(std::io::Error::other(ReadError::Expired)),
        result = tokio::time::timeout_at(deadline, async {
            output.write_all(&bytes).await?;
            output.flush().await
        }) => result.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "MCP output deadline"))?,
    }
}

fn expiry_deadline(expires_at: u64) -> std::io::Result<tokio::time::Instant> {
    // Capture monotonic time first so mapping the exact wall-clock boundary
    // cannot extend the lease through sampling overhead or fractional seconds.
    let now = tokio::time::Instant::now();
    let remaining = UNIX_EPOCH
        .checked_add(Duration::from_secs(expires_at))
        .and_then(|expiry| expiry.duration_since(SystemTime::now()).ok())
        .unwrap_or_default();
    now.checked_add(remaining)
        .ok_or_else(|| std::io::Error::other(ReadError::Expired))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

    fn reader() -> ScopedReader {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let view = serde_json::from_value(json!({
            "version":1, "id":"deadline-view", "policy_revision":"v1",
            "identity":{"company":"c", "project":"p", "agent":"a", "task":"t", "run":"r", "host":"h", "server":"s"},
            "repo":"repo", "build_id":"build", "snapshots":["snapshot"],
            "source_paths":["."], "excluded_paths":[], "operations":["status"],
            "issued_at":now, "expires_at":now+300,
            "budgets":{ "max_nodes":100, "max_edges":100, "max_graph_bytes":1_048_576,
                "max_evidence":20, "max_page_size":20, "max_request_bytes":8192,
                "max_response_bytes":65536, "timeout_ms":1000, "max_calls":30,
                "max_hops":4, "max_visits":100 }
        })).unwrap();
        let (tx, _) = tokio::sync::watch::channel(false);
        ScopedReader {
            view,
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            evidence: BTreeMap::new(),
            revoker: super::super::Revoker(tx),
            calls: AtomicUsize::new(0),
        }
    }

    #[tokio::test]
    async fn queued_call_and_backpressured_output_share_one_deadline() {
        let reader = Arc::new(reader());
        let accepted = tokio::time::Instant::now();
        let call = call_task(
            &reader,
            &json!(1),
            &json!({"name":"status", "arguments":{"repo":"repo"}}),
        );
        // Executor scheduling is part of the request budget. Spend most of
        // it before collecting the result, then prevent any output progress.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let response = call.task.await.unwrap();
        let (mut output, _unread_client) = tokio::io::duplex(1);
        assert!(write(
            &mut output,
            &response,
            &reader,
            expiry_deadline(reader.view.expires_at).unwrap(),
            Some(call.deadline)
        )
        .await
        .is_err());
        assert!(
            accepted.elapsed() <= Duration::from_millis(1100),
            "output reset the request deadline: {:?}",
            accepted.elapsed()
        );
    }

    #[tokio::test]
    async fn expiry_interrupts_a_backpressured_write_at_the_exact_boundary() {
        let mut reader = reader();
        // Begin well into a Unix second so flooring wall time would grant
        // almost a full second past the exclusive expiry boundary.
        loop {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            if (600..750).contains(&now.subsec_millis()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        reader.view.expires_at = now.as_secs() + 1;
        reader.view.budgets.timeout_ms = 5000;
        let boundary = UNIX_EPOCH + Duration::from_secs(reader.view.expires_at);
        let (mut output, _unread_client) = tokio::io::duplex(1);
        assert!(write(
            &mut output,
            &json!({"id":1}),
            &reader,
            expiry_deadline(reader.view.expires_at).unwrap(),
            None
        )
        .await
        .is_err());
        let late = SystemTime::now()
            .duration_since(boundary)
            .unwrap_or_default();
        assert!(
            late < Duration::from_millis(150),
            "write outlived expiry by {late:?}"
        );
    }
}
