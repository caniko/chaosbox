//! Read-only MCP over stdio: tool definitions, argument validation, dispatch, and the serve loop.

use super::{
    AnyReader, LifecycleReport, PgReader, EXPORT_NODE_CAP, EXPORT_EDGE_CAP, all_relation_types,
};
use std::{
    collections::VecDeque,
    future::{Future, pending},
    time::Duration,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

// ---- Read-only MCP (JSON-RPC over stdio, full handshake) ----

/// MCP protocol version served here.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Older protocol versions still accepted from clients.
const MCP_PROTOCOL_FALLBACKS: &[&str] = &["2024-11-05", "2025-03-26"];
/// Tools per `tools/list` page.
const MCP_PAGE_SIZE: usize = 5;
/// Upper bound for caller-supplied search limits: paginate instead.
const MCP_SEARCH_LIMIT_MAX: i64 = 200;
/// Limits include framing bytes. Overlong input closes the connection.
pub(super) const MCP_FRAME_BYTES: usize = 64 * 1024;
pub(super) const MCP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MCP_OUTPUT_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_GRAPH_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MCP_PENDING_REQUESTS: usize = 8;
const MCP_PENDING_BYTES: usize = 256 * 1024;
// Admission wait is separate from dispatch-owned backend/path deadlines.
const MCP_ADMISSION_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn mcp_tool_defs() -> Vec<serde_json::Value> {
    vec![
        mcp_tool(
            "search",
            "Substring search over entity names (sorted, bounded).",
            serde_json::json!({"query": {"type": "string"}, "limit": {"type": "integer", "default": 20, "maximum": 200}}),
            vec!["query"],
        ),
        mcp_tool(
            "lookup",
            "Typed entity lookup by id. Optional build_id selects an exact repository-owned publication; omission selects current at dispatch.",
            serde_json::json!({"id": {"type": "string"}}),
            vec!["id"],
        ),
        {
            let mut tool = mcp_tool(
                "neighbors",
                "Page neighborhoods in relationship-ID order. Limit applies per direction. Follow next_cursor with the same arguments until complete; continuation pins the exact build. Optional build_id starts a fresh historical anchor and must agree with any cursor. Use rel or rel_types, not both; an empty rel_types list matches nothing.",
                serde_json::json!({"id":{"type":"string","minLength":1,"maxLength":256},
                    "rel":{"type":"string"},"rel_types":{"type":"array","items":{"type":"string"}},
                    "limit":{"type":"integer","minimum":1,"maximum":200,"default":super::backend_cmd::NEIGHBORHOOD_DEFAULT_LIMIT},
                    "cursor":{"type":"string","maxLength":16384},
                    "direction":{"type":"string","enum":["both","outgoing","incoming"],"default":"both"}}),
                vec!["id"],
            );
            tool["inputSchema"]["additionalProperties"] = serde_json::json!(false);
            tool["inputSchema"]["not"] = serde_json::json!({"required":["rel","rel_types"]});
            tool
        },
        {
            let text = serde_json::json!({"type":"string","minLength":1,"maxLength":256});
            let mut tool = mcp_tool(
                "path",
                "Shortest undirected path within max_hops; null when absent, error on work/deadline exhaustion. Repository and endpoints must be nonblank, control-free and at most 256 UTF-8 bytes.",
                serde_json::json!({"from":text,"to":text,
                    "max_hops":{"type":"integer","minimum":0,
                        "default":chaosbox::PathQuery::DEFAULT_MAX_HOPS,
                        "maximum":chaosbox::PathQuery::MAX_HOPS}}),
                vec!["from", "to"],
            );
            tool["inputSchema"]["properties"]["repo"] = text;
            tool["inputSchema"]["additionalProperties"] = serde_json::json!(false);
            tool
        },
        mcp_tool(
            "evidence",
            "Claim evidence and source locations for a relationship.",
            serde_json::json!({"rel": {"type": "string"}}),
            vec!["rel"],
        ),
        mcp_tool(
            "status",
            "Active-build status, coverage, and generation.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "diff",
            "Node/edge id diff between two builds of one repo.",
            serde_json::json!({"from_build": {"type": "string"}, "to_build": {"type": "string"}}),
            vec!["from_build", "to_build"],
        ),
        mcp_tool(
            "export",
            "Deterministic export of the pinned active build.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "explain",
            "Source-backed entity explanation (no generated prose).",
            serde_json::json!({"id": {"type": "string"}}),
            vec!["id"],
        ),
        mcp_tool(
            "context",
            "Bounded lexical graph context with pinned source and relationship identities.",
            serde_json::json!({"query":{"type":"string","minLength":1,"maxLength":2048},"depth":{"type":"integer","minimum":1,"maximum":6},"max_nodes":{"type":"integer","minimum":1,"maximum":200},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}),
            vec!["query"],
        ),
        mcp_tool(
            "stats",
            "Deterministic counts, degree hubs and weak connectivity groups (not semantic clusters).",
            serde_json::json!({"limit":{"type":"integer","minimum":1,"maximum":200}}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "community",
            "Build-bound weak connectivity group, with explicit omitted members.",
            serde_json::json!({"id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}),
            vec!["id"],
        ),
    ]
}

// properties/required move into the schema json! below, which the
// pass-by-value lint cannot see through (macro boundary false positive).
#[allow(clippy::needless_pass_by_value)]
pub(super) fn mcp_tool(
    name: &str,
    description: &str,
    properties: serde_json::Value,
    required: Vec<&str>,
) -> serde_json::Value {
    let mut required = required;
    // Every tool requires an explicit repo: the server holds no default
    // (a silent `demo` fallback once sent agents to the wrong graph).
    // Declared here, not per tool, so schema and runtime validation agree.
    if !required.contains(&"repo") {
        required.push("repo");
    }
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.insert(
            "repo".to_owned(),
            serde_json::json!({"type": "string", "minLength": 1}),
        );
        if matches!(name, "lookup" | "explain" | "evidence" | "neighbors") {
            props.insert("build_id".into(), serde_json::json!({
                "type":"string", "minLength":1, "maxLength":256,
                "description":"Exact repository-owned publication. Omit for current; with a cursor, must match its build."
            }));
        }
    }
    serde_json::json!({
        "name": name, "description": description,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": true},
    })
}

pub(super) fn mcp_text_result(
    id: &serde_json::Value,
    payload: &serde_json::Value,
) -> serde_json::Value {
    let Ok(bytes) = chaosbox::read_view::bounded_json(payload, MCP_RESPONSE_BYTES) else {
        return mcp_error(id, -32603, "MCP response byte budget exceeded".into(), None);
    };
    serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {"content": [{"type": "text", "text": String::from_utf8_lossy(&bytes)}]},
    })
}

// message moves into the error json! below (macro boundary false positive
// for the pass-by-value lint, same as mcp_tool above).
#[allow(clippy::needless_pass_by_value)]
pub(super) fn mcp_error(
    id: &serde_json::Value,
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut error = serde_json::json!({"code": code, "message": message});
    if let Some(d) = data {
        error["data"] = d;
    }
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// Validate tool arguments against required fields (types are checked per tool).
pub(super) fn mcp_args(
    name: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, serde_json::Value> {
    let args = params.get("arguments").unwrap_or(&serde_json::Value::Null);
    let map = args.as_object().cloned().unwrap_or_default();
    let required: &[&str] = match name {
        "search" | "context" => &["query"],
        "lookup" | "neighbors" | "explain" | "community" => &["id"],
        "path" => &["from", "to"],
        "evidence" => &["rel"],
        "diff" => &["from_build", "to_build"],
        _ => &[],
    };
    for key in required {
        if map
            .get(*key)
            .and_then(|v| v.as_str())
            .is_none_or(str::is_empty)
        {
            return Err(
                serde_json::json!({"code": -32602, "message": format!("missing required argument: {key}")}),
            );
        }
    }
    Ok(map)
}

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
async fn mcp_call_tool_connected(
    id: &serde_json::Value,
    name: &str,
    params: &serde_json::Value,
    connection: &mut Option<PgReader>,
) -> serde_json::Value {
    let args = match mcp_args(name, params) {
        Ok(a) => a,
        Err(e) => {
            let code = e["code"].as_i64().unwrap_or(-32602);
            let msg = e["message"].as_str().unwrap_or("invalid params").to_owned();
            return mcp_error(id, code, msg, None);
        }
    };
    // Closed read-only tool set: reject unknown (write/mutation) tools before
    // touching the backend or credentials of any kind.
    if !matches!(
        name,
        "search"
            | "lookup"
            | "neighbors"
            | "path"
            | "evidence"
            | "status"
            | "diff"
            | "export"
            | "explain"
            | "context"
            | "stats"
            | "community"
    ) {
        return mcp_error(
            id,
            -32601,
            format!("read-only MCP: no such tool (rejected): {name}"),
            None,
        );
    }
    // Explicit repository: fail before touching the backend or credentials.
    let Some(repo) = args
        .get("repo")
        .and_then(|r| r.as_str())
        .filter(|r| !r.is_empty())
    else {
        return mcp_error(
            id,
            -32602,
            "missing required argument: repo".to_owned(),
            None,
        );
    };
    let build_id = args
        .get("build_id")
        .map(|value| {
            if !matches!(name, "lookup" | "explain" | "evidence" | "neighbors") {
                return Err("build_id is not supported by this tool".to_owned());
            }
            let build_id = value.as_str().ok_or("build_id must be a string")?;
            chaosbox::validate_build_id(build_id).map_err(|error| error.to_string())?;
            Ok(build_id)
        })
        .transpose();
    let build_id = match build_id {
        Ok(build_id) => build_id,
        Err(error) => return mcp_error(id, -32602, error, None),
    };
    // Continuation selects its historical publication before any active pin.
    if name == "neighbors" {
        let query = match prepare_neighborhood(args) {
            Ok(query) => query,
            Err(error) => return mcp_error(id, -32602, error, None),
        };
        return match async {
            query
                .read(AnyReader::handle_cached(connection).await?)
                .await
        }
        .await
        {
            Ok(page) => mcp_text_result(id, &serde_json::json!(page)),
            Err(error) => mcp_backend_error(id, name, &error),
        };
    }
    if name == "path" {
        let query = serde_json::from_value::<chaosbox::PathQuery>(serde_json::Value::Object(args))
            .map_err(|error| error.to_string())
            .and_then(|query| query.prepare().map_err(|error| error.to_string()));
        let query = match query {
            Ok(query) => query,
            Err(error) => return mcp_error(id, -32602, error, None),
        };
        return match query.read(AnyReader::handle_cached(connection)).await {
            Ok(value) => mcp_text_result(id, &value),
            Err(error) => mcp_backend_error(id, name, &error),
        };
    }
    // Cheap service limits before any backend work.
    if name == "search" {
        if let Some(l) = args.get("limit").and_then(serde_json::Value::as_i64) {
            if l > MCP_SEARCH_LIMIT_MAX {
                return mcp_error(
                    id,
                    -32602,
                    format!("search limit exceeds maximum {MCP_SEARCH_LIMIT_MAX}"),
                    None,
                );
            }
        }
    }
    let bounds: &[(&str, u64, u64)] = match name {
        "context" => &[
            ("depth", 1, 6),
            ("max_nodes", 1, 200),
            ("max_chars", 256, 32_000),
        ],
        "stats" | "community" => &[("limit", 1, 200)],
        _ => &[],
    };
    for (key, minimum, maximum) in bounds {
        if args.get(*key).is_some_and(|value| {
            !value
                .as_u64()
                .is_some_and(|n| (*minimum..=*maximum).contains(&n))
        }) {
            return mcp_error(
                id,
                -32602,
                format!("{key} must be an integer in {minimum}..{maximum}"),
                None,
            );
        }
    }
    if name == "context"
        && args["query"]
            .as_str()
            .is_none_or(|query| query.trim().is_empty() || query.len() > 2048)
    {
        return mcp_error(
            id,
            -32602,
            "context query must be 1..2048 bytes".into(),
            None,
        );
    }
    let reader = match async {
        AnyReader::pin(AnyReader::handle_cached(connection).await?, repo, build_id).await
    }
    .await
    {
        Ok(r) => r,
        Err(e) => return mcp_backend_error(id, name, &e),
    };
    let payload: Result<serde_json::Value, String> = match name {
        "search" => {
            let q = args["query"].as_str().unwrap_or_default();
            let limit = args
                .get("limit")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(20);
            reader
                .search(q, limit)
                .await
                .map(|rows| serde_json::to_value(&rows).unwrap())
                .map_err(|e| e.to_string())
        }
        "lookup" => {
            let eid = args["id"].as_str().unwrap_or_default();
            reader
                .lookup(eid)
                .await
                .map(|row| serde_json::to_value(&row).unwrap())
                .map_err(|e| e.to_string())
        }
        "evidence" => {
            let rel = args["rel"].as_str().unwrap_or_default();
            reader.evidence(rel).await.map_err(|e| e.to_string())
        }
        "status" => Ok(serde_json::json!({
            "repo": repo, "build_id": reader.build_id(), "generation": reader.generation(),
            "status": reader.status(), "snapshots": reader.snapshots(),
            "coverage": reader.coverage.as_ref().map(chaosbox_core::coverage::BuildCoverage::report),
            "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
        })),
        "diff" => {
            let from = args["from_build"].as_str().unwrap_or_default();
            let to = args["to_build"].as_str().unwrap_or_default();
            reader.diff(repo, from, to).await.map_err(|e| e.to_string())
        }
        "export" => reader.export().await.map_err(|e| e.to_string()),
        "context" => reader
            .context(
                args["query"].as_str().unwrap_or_default(),
                integer(args.get("depth"), 3),
                integer(args.get("max_nodes"), 20),
                integer(args.get("max_chars"), 12000),
            )
            .await
            .map_err(|e| e.to_string()),
        "stats" => reader
            .stats(integer(args.get("limit"), 10))
            .await
            .map_err(|e| e.to_string()),
        "community" => reader
            .community(
                args["id"].as_str().unwrap_or_default(),
                integer(args.get("limit"), 100),
            )
            .await
            .map_err(|e| e.to_string()),
        "explain" => {
            let eid = args["id"].as_str().unwrap_or_default();
            reader.explain(eid).await.map_err(|e| e.to_string())
        }
        _ => {
            return mcp_error(
                id,
                -32601,
                format!("read-only MCP: no such tool (rejected): {name}"),
                None,
            );
        }
    };
    match payload {
        Ok(v) => mcp_text_result(id, &v),
        Err(e) => mcp_error(id, -32603, e, None),
    }
}

#[cfg(test)]
pub(super) async fn mcp_call_tool(id: &Value, name: &str, params: &Value) -> Value {
    mcp_call_tool_connected(id, name, params, &mut None).await
}

/// Read-only MCP over stdio: full handshake, paginated tools, validated calls.
/// Never loads Jev credentials; never accepts prose as evidence.
// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
pub(super) async fn serve_mcp(
    intelligence: Option<chaosbox::intelligence::Bundle>,
    workspace: Option<chaosbox::workspace::Workspace>,
    mut current: Option<chaosbox::sync::current::CurrentReader>,
    federation: Option<chaosbox::federation::Federator>,
) -> std::io::Result<()> {
    let mut stdout = mcp_output()?;
    let mut frames = Frames::new(tokio::io::stdin());
    let mut initialized = false;
    // ponytail: one idle driver per ordinary server; no result/header cache.
    // Queued frames acquire credentials and fresh pins only when dispatched.
    let mut graph: Option<(PgReader, tokio::time::Instant)> = None;
    loop {
        let line = tokio::select! {
            biased;
            () = async { match graph.as_ref() {
                Some((_, deadline)) => tokio::time::sleep_until(*deadline).await,
                None => pending().await,
            }} => { graph = None; continue; }
            line = frames.request() => line?,
        };
        let Some((line, admission_deadline)) = line else {
            break;
        };
        let req = match decode_frame(&line) {
            Ok(Some(req)) => req,
            Ok(None) => continue,
            Err(response) => {
                if !exchange(&mut frames, &mut stdout, &Value::Null, async { response }).await? {
                    break;
                }
                continue;
            }
        };
        // Notifications carry no id and get no response.
        let id = match req.get("id") {
            Some(i) => i.clone(),
            None => continue,
        };
        if admission_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            if !exchange(&mut frames, &mut stdout, &id, async {
                admission_expired(&id)
            })
            .await?
            {
                break;
            }
            continue;
        }
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut response = async || match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let version = if requested == MCP_PROTOCOL_VERSION
                    || MCP_PROTOCOL_FALLBACKS.contains(&requested)
                {
                    requested.to_owned()
                } else {
                    MCP_PROTOCOL_VERSION.to_owned()
                };
                initialized = true;
                serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "chaosbox", "version": env!("CARGO_PKG_VERSION")},
                    },
                })
            }
            "ping" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => {
                if initialized {
                    let mut defs = mcp_tool_defs();
                    if intelligence.is_some() || current.is_some() || federation.is_some() {
                        defs.push(mcp_tool("intelligence_context", "Small historical, source-backed knowledge packet; not instructions or current-state proof.", serde_json::json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}), vec!["query"]));
                        if federation.is_some() {
                            defs.push(mcp_tool(
                                "intelligence_evidence",
                                "Authorized source quotes and receipt projections from the exact originating provider/snapshot.",
                                serde_json::json!({"handle":{"type":"object","additionalProperties":false,"properties":{
                                    "provider":{"type":"string"},"owner":{"type":"string"},"scope":{"type":"string"},
                                    "project":{"type":"string"},"snapshot":{"type":"string"},"id":{"type":"string"}},
                                    "required":["provider","owner","scope","project","snapshot","id"]},
                                    "max_chars":{"type":"integer","minimum":256,"maximum":32000}}),
                                vec!["handle"],
                            ));
                        } else {
                            defs.push(mcp_tool(
                                "intelligence_evidence",
                                "Sources and typed decision receipts for a pinned intelligence item.",
                                serde_json::json!({"id":{"type":"string"}}),
                                vec!["id"],
                            ));
                        }
                    }
                    if workspace.is_some() {
                        defs.push(mcp_tool("workspace_impact", "Reviewed directed impact, with current freshness, exact pins and private scope checks.",
                            serde_json::json!({"changed":{"type":"string"},"scope":{"type":"string"},"max_hops":{"type":"integer","minimum":1,"maximum":8},"max_nodes":{"type":"integer","minimum":1,"maximum":100}}), vec!["changed", "scope"]));
                    }
                    let cursor = params
                        .get("cursor")
                        .and_then(|c| c.as_str())
                        .and_then(|c| c.parse::<usize>().ok())
                        .unwrap_or(0);
                    let page: Vec<_> = defs.into_iter().skip(cursor).take(MCP_PAGE_SIZE).collect();
                    let next = if page.len() == MCP_PAGE_SIZE {
                        Some((cursor + MCP_PAGE_SIZE).to_string())
                    } else {
                        None
                    };
                    let mut result = serde_json::json!({"tools": page});
                    if let Some(n) = next {
                        result["nextCursor"] = serde_json::Value::String(n);
                    }
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            "tools/call" => {
                if initialized {
                    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name == "workspace_impact" {
                        graph = None;
                        match workspace.as_ref() {
                            Some(workspace) => {
                                match workspace_call(workspace, &params["arguments"]) {
                                    Ok(value) => mcp_text_result(&id, &value),
                                    Err(error) => mcp_error(&id, -32602, error, None),
                                }
                            }
                            None => mcp_error(
                                &id,
                                -32601,
                                "no workspace artifact configured".into(),
                                None,
                            ),
                        }
                    } else if name.starts_with("intelligence_") {
                        graph = None;
                        if let Some(reader) = federation.as_ref() {
                            match reader.query(name, &params["arguments"]).await {
                                Ok(value) => mcp_text_result(&id, &value),
                                Err(error) => mcp_error(&id, -32602, error.to_string(), None),
                            }
                        } else if let Some(reader) = current.as_mut() {
                            match reader.query(name, &params["arguments"]).await {
                                Ok(value) => mcp_text_result(&id, &value),
                                Err(error) => mcp_error(&id, -32602, error, None),
                            }
                        } else {
                            match intelligence.as_ref() {
                                Some(bundle) => match chaosbox::intelligence::mcp_query(
                                    bundle,
                                    name,
                                    &params["arguments"],
                                ) {
                                    Ok(value) => mcp_text_result(&id, &value),
                                    Err(error) => mcp_error(&id, -32602, error, None),
                                },
                                None => mcp_error(
                                    &id,
                                    -32601,
                                    "no intelligence bundle configured".into(),
                                    None,
                                ),
                            }
                        }
                    } else {
                        // Move ownership into this future. Cancellation drops all
                        // request-owned clones; no cached driver survives it.
                        let mut connection = graph
                            .take()
                            .filter(|(_, deadline)| tokio::time::Instant::now() < *deadline)
                            .map(|(handle, _)| handle);
                        let result =
                            mcp_call_tool_connected(&id, name, &params, &mut connection).await;
                        if result.get("error").is_none() {
                            graph = connection.map(|handle| {
                                (handle, tokio::time::Instant::now() + MCP_GRAPH_IDLE_TIMEOUT)
                            });
                        }
                        result
                    }
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            _ => mcp_error(
                &id,
                -32601,
                format!("unknown method (rejected): {method}"),
                None,
            ),
        };
        if !exchange(&mut frames, &mut stdout, &id, response()).await? {
            break;
        }
    }
    let _ = all_relation_types;
    Ok(())
}

fn mcp_output() -> std::io::Result<Box<dyn AsyncWrite + Unpin>> {
    // Tokio Stdout uses an uncancellable blocking write. Reopen Linux pipes
    // nonblocking so EOF can stop an unread response without stranding a worker
    // during runtime shutdown. Keep normal terminal/file redirection support.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::FileTypeExt;
        if std::fs::metadata("/proc/self/fd/1")?.file_type().is_fifo() {
            return Ok(Box::new(
                tokio::net::unix::pipe::OpenOptions::new().open_sender("/proc/self/fd/1")?,
            ));
        }
    }
    Ok(Box::new(tokio::io::stdout()))
}

struct Queued {
    line: Vec<u8>,
    id: Value,
    deadline: tokio::time::Instant,
}

/// Cancellation-safe framing and bounded raw-request admission. No queued frame
/// owns credentials, a backend future, a publication pin or a query result.
pub(super) struct Frames<I> {
    input: BufReader<I>,
    line: Vec<u8>,
    queued: VecDeque<Queued>,
}

impl<I: AsyncRead + Unpin> Frames<I> {
    pub(super) fn new(input: I) -> Self {
        Self {
            input: BufReader::new(input),
            line: Vec::new(),
            queued: VecDeque::new(),
        }
    }

    async fn request(
        &mut self,
    ) -> std::io::Result<Option<(Vec<u8>, Option<tokio::time::Instant>)>> {
        if let Some(queued) = self.queued.pop_front() {
            return Ok(Some((queued.line, Some(queued.deadline))));
        }
        Ok(self.next().await?.map(|line| (line, None)))
    }

    pub(super) async fn next(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let remaining = MCP_FRAME_BYTES + 1 - self.line.len();
        let read = (&mut self.input)
            .take(remaining as u64)
            .read_until(b'\n', &mut self.line)
            .await?;
        if self.line.len() > MCP_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP input frame byte budget exceeded",
            ));
        }
        if read == 0 && self.line.is_empty() {
            return Ok(None);
        }
        if !self.line.ends_with(b"\n") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete MCP input frame",
            ));
        }
        Ok(Some(std::mem::take(&mut self.line)))
    }
}

fn decode_frame(line: &[u8]) -> Result<Option<Value>, Value> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let req: Value = serde_json::from_slice(line)
        .map_err(|_| mcp_error(&Value::Null, -32700, "parse error".into(), None))?;
    if !req.is_object()
        || req["jsonrpc"] != "2.0"
        || !req["method"].is_string()
        || req.get("id").is_some_and(|id| {
            !(id.is_i64()
                || id.is_u64()
                || id
                    .as_str()
                    .is_some_and(|s| s.len() <= 128 && !s.chars().any(char::is_control)))
        })
    {
        return Err(mcp_error(
            &Value::Null,
            -32600,
            "invalid request".into(),
            None,
        ));
    }
    Ok(Some(req))
}

struct Output {
    id: Value,
    bytes: Vec<u8>,
    offset: usize,
    deadline: tokio::time::Instant,
    final_response: bool,
}

impl Output {
    fn new(response: &Value, final_response: bool) -> Self {
        let mut bytes = chaosbox::read_view::bounded_json(response, MCP_RESPONSE_BYTES - 1)
            .unwrap_or_else(|_| {
                mcp_error(
                    &response["id"],
                    -32603,
                    "MCP response byte budget exceeded".into(),
                    None,
                )
                .to_string()
                .into_bytes()
            });
        bytes.push(b'\n');
        Self {
            id: response["id"].clone(),
            bytes,
            offset: 0,
            deadline: tokio::time::Instant::now() + MCP_OUTPUT_TIMEOUT,
            final_response,
        }
    }

    /// `write`, unlike `write_all`, is cancellation safe. Keep the offset and
    /// deadline outside the future so input cannot replay bytes or extend a write.
    async fn step<O: AsyncWrite + Unpin>(&mut self, output: &mut O) -> std::io::Result<bool> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "MCP output deadline exceeded",
            ));
        }
        tokio::time::timeout_at(self.deadline, async {
            if self.offset == self.bytes.len() {
                output.flush().await?;
                return Ok(true);
            }
            let written = output.write(&self.bytes[self.offset..]).await?;
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            self.offset += written;
            Ok(false)
        })
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "MCP output deadline exceeded")
        })?
    }
}

fn admission_expired(id: &Value) -> Value {
    mcp_error(
        id,
        -32000,
        "MCP admission wait deadline exceeded".into(),
        None,
    )
}

/// One backend future, one final response and bounded request/control queues.
/// EOF/error drops the in-place backend future; pings and cancellation bypass
/// admission. Controls drain before returning so every removed request receives
/// its response even when the active final response was already being written.
// ponytail: one active backend request per connection; increase concurrency only
// with explicit per-request connection ownership and the same cleanup bounds.
pub(super) async fn exchange<I, O, F>(
    frames: &mut Frames<I>,
    output: &mut O,
    id: &Value,
    response: F,
) -> std::io::Result<bool>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    F: Future<Output = Value>,
{
    let mut call = Some(Box::pin(response));
    let mut writing: Option<Output> = None;
    let mut ready: Option<Output> = None;
    let mut controls = VecDeque::<Output>::new();
    let mut finished = false;
    loop {
        if writing.is_none() {
            writing = controls.pop_front().or_else(|| ready.take());
        }
        if finished && writing.is_none() {
            return Ok(true);
        }
        let deadline = frames.queued.front().map(|queued| queued.deadline);
        let output_deadline = ready.as_ref().map(|frame| frame.deadline);
        tokio::select! {
            biased;
            value = async { match call.as_mut() { Some(future) => future.await, None => pending().await } } => {
                call = None;
                ready = Some(Output::new(&value, true));
            }
            written = async { match writing.as_mut() { Some(frame) => frame.step(output).await, None => pending().await } } => {
                if written? {
                    finished |= writing.take().is_some_and(|frame| frame.final_response);
                }
            }
            () = async { match output_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => pending().await,
            }} => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "MCP output deadline exceeded")),
            () = async { match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => pending().await,
            }} => {
                if controls.len() == MCP_PENDING_REQUESTS {
                    return Err(std::io::Error::other("MCP output backpressure: response queue full"));
                }
                if let Some(queued) = frames.queued.pop_front() {
                    controls.push_back(Output::new(&admission_expired(&queued.id), false));
                }
            }
            line = frames.next() => {
                let Some(line) = line? else { return Ok(false); };
                let response = match decode_frame(&line) {
                    Ok(None) => continue,
                    Err(error) => error,
                    Ok(Some(req)) => {
                        if req.get("id").is_none() {
                            if req["method"] != "notifications/cancelled" { continue; }
                            let Some(target) = req["params"].get("requestId") else { continue; };
                            if call.is_some() && target == id {
                                call = None;
                                ready = Some(Output::new(&mcp_error(id, -32800, "request cancelled".into(), None), true));
                                continue;
                            }
                            let Some(position) = frames.queued.iter().position(|queued| queued.id == *target) else { continue; };
                            let queued = frames.queued.remove(position).expect("queued position exists");
                            mcp_error(&queued.id, -32800, "request cancelled".into(), None)
                        } else if (!finished && req["id"] == *id)
                            || frames.queued.iter().any(|queued| queued.id == req["id"])
                            || writing.as_ref().is_some_and(|frame| frame.id == req["id"])
                            || controls.iter().any(|frame| frame.id == req["id"]) {
                            mcp_error(&Value::Null, -32600, "duplicate outstanding request id".into(), None)
                        } else if req["method"] == "ping" {
                            serde_json::json!({"jsonrpc":"2.0", "id":req["id"], "result":{}})
                        } else if frames.queued.len() < MCP_PENDING_REQUESTS
                            && frames.queued.iter().map(|queued| queued.line.len()).sum::<usize>() + line.len() <= MCP_PENDING_BYTES {
                            frames.queued.push_back(Queued { line, id: req["id"].clone(),
                                deadline: tokio::time::Instant::now() + MCP_ADMISSION_TIMEOUT });
                            continue;
                        } else {
                            mcp_error(&req["id"], -32000, "MCP admission queue full".into(), None)
                        }
                    }
                };
                if controls.len() == MCP_PENDING_REQUESTS {
                    return Err(std::io::Error::other("MCP output backpressure: response queue full"));
                }
                controls.push_back(Output::new(&response, false));
            }
        }
    }
}

fn workspace_call(
    workspace: &chaosbox::workspace::Workspace,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let scope = args["scope"].as_str().ok_or("missing scope")?;
    let repo = args["repo"].as_str().ok_or("missing repo")?;
    let changed = args["changed"].as_str().ok_or("missing changed endpoint")?;
    if workspace
        .endpoints
        .get(changed)
        .is_none_or(|endpoint| endpoint.member != repo)
    {
        return Err("changed endpoint is outside requested repository".into());
    }
    for (key, maximum) in [("max_hops", 8), ("max_nodes", 100)] {
        if args
            .get(key)
            .is_some_and(|value| !value.as_u64().is_some_and(|n| (1..=maximum).contains(&n)))
        {
            return Err(format!("{key} must be an integer in 1..{maximum}"));
        }
    }
    workspace
        .impact(
            scope,
            changed,
            integer(args.get("max_hops"), 4),
            integer(args.get("max_nodes"), 20),
        )
        .map_err(|e| e.to_string())
}

fn prepare_neighborhood(
    mut args: serde_json::Map<String, serde_json::Value>,
) -> Result<chaosbox::PreparedNeighborhood, String> {
    if let Some(rel) = args.remove("rel") {
        if args.contains_key("rel_types") {
            return Err("rel and rel_types are mutually exclusive".into());
        }
        let rel = rel.as_str().ok_or("rel must be a string")?;
        args.insert("rel_types".into(), serde_json::json!([rel]));
    }
    for (key, valid) in [
        (
            "cursor",
            args.get("cursor").is_none_or(serde_json::Value::is_string),
        ),
        (
            "rel_types",
            args.get("rel_types")
                .is_none_or(serde_json::Value::is_array),
        ),
    ] {
        if !valid {
            return Err(format!("{key} has an invalid type"));
        }
    }
    args.entry("limit").or_insert(serde_json::json!(
        super::backend_cmd::NEIGHBORHOOD_DEFAULT_LIMIT
    ));
    let query: chaosbox::NeighborhoodQuery =
        serde_json::from_value(serde_json::Value::Object(args))
            .map_err(|error| error.to_string())?;
    query.prepare().map_err(|error| error.to_string())
}

fn mcp_backend_error(
    id: &serde_json::Value,
    name: &str,
    error: &impl std::fmt::Display,
) -> serde_json::Value {
    let message = error.to_string();
    let mut data = serde_json::json!(LifecycleReport::error(&format!("mcp {name}"), &message));
    // Machine-readable class for the one failure agents must act on
    // differently (index or pick another repo key) instead of retrying.
    if message.contains("no active build for repo") {
        data["code"] = serde_json::json!("no_active_build");
    }
    mcp_error(id, -32603, message, Some(data))
}

fn integer(value: Option<&serde_json::Value>, default: usize) -> usize {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(default)
}
