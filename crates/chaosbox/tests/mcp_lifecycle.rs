//! Ordinary stdio MCP must observe disconnects and cancellation during acquisition.

#[path = "../../chaosbox-pg/tests/support/server.rs"]
mod server;

use std::{process::Stdio, time::Duration};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};

const REPO: &str = "mcp-lifecycle";
const SNAP: &str = "s-mcp-lifecycle";
/// Nothing listens here: acquisition fails immediately.
const REFUSED: &str = "host=127.0.0.1 port=1 dbname=refused user=refused";

/// A loopback listener that accepts TCP but never answers the startup message.
fn stalled(listener: &tokio::net::TcpListener) -> String {
    format!(
        "host=127.0.0.1 port={} dbname=stalled user=stalled",
        listener.local_addr().unwrap().port()
    )
}

struct Client {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: Lines<BufReader<tokio::process::ChildStdout>>,
}

impl Client {
    async fn start(url: &str) -> Self {
        let binary = std::env::var_os("CHAOSBOX_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_chaosbox").into());
        let mut child = tokio::process::Command::new(binary)
            .arg("mcp")
            .env("CHAOSBOX_DATABASE_URL", url)
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Self {
            child,
            input,
            output,
        };
        client
            .send(json!({"jsonrpc":"2.0","id":0,"method":"initialize"}))
            .await;
        assert!(client.receive().await.get("result").is_some());
        client
    }

    async fn send(&mut self, value: Value) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        self.receive_within(Duration::from_secs(2)).await
    }

    async fn receive_within(&mut self, timeout: Duration) -> Value {
        let line = tokio::time::timeout(timeout, self.output.next_line())
            .await
            .expect("ordinary MCP stopped reading input")
            .unwrap()
            .expect("MCP closed early");
        serde_json::from_str(&line).unwrap()
    }

    async fn path(&mut self, id: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":"path","arguments":{"repo":"demo","from":"ent:a","to":"ent:b"}}}))
            .await;
    }

    async fn stopped(&mut self) -> std::process::ExitStatus {
        tokio::time::timeout(Duration::from_secs(2), self.child.wait())
            .await
            .expect("MCP retained an active request after disconnect")
            .unwrap()
    }
}

#[tokio::test]
async fn matching_cancel_and_eof_close_real_acquisition_sockets() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = Client::start(&stalled(&listener)).await;
    client.path(json!(7)).await;
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    // Prove a real connector owns the socket before cancelling it: a loaded
    // build sandbox can otherwise cancel before the startup message is written.
    let mut startup = [0_u8; 1];
    tokio::time::timeout(Duration::from_secs(10), socket.read_exact(&mut startup))
        .await
        .expect("real connector never wrote its startup message")
        .unwrap();
    // JSON-RPC string and numeric IDs are distinct. A ping proves that the
    // unrelated cancellation has been consumed while the connector is pending.
    client
        .send(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"7"}}),
        )
        .await;
    client
        .send(json!({"jsonrpc":"2.0","id":8,"method":"ping"}))
        .await;
    assert_eq!(client.receive().await["id"], 8);
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}))
        .await;
    let cancelled = client.receive().await;
    assert_eq!(cancelled["id"], 7);
    assert_eq!(cancelled["error"]["code"], -32800);
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut Vec::new()))
        .await
        .expect("cancelled acquisition socket stayed open")
        .unwrap();
    client.path(json!("next")).await;
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    drop(client.input.take());
    assert!(client.stopped().await.success());
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut Vec::new()))
        .await
        .expect("EOF acquisition socket stayed open")
        .unwrap();
}

#[tokio::test]
async fn overlapping_calls_serialize_and_queued_cancellation_preserves_typed_ids() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = Client::start(&stalled(&listener)).await;
    client.path(json!(7)).await;
    let (mut socket, _) = listener.accept().await.unwrap();
    for id in [json!(8), json!("8"), json!(10)] {
        client
            .send(json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}))
            .await;
    }
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":8}}))
        .await;
    client
        .send(json!({"jsonrpc":"2.0","id":9,"method":"ping"}))
        .await;
    let cancelled = client.receive().await;
    assert_eq!(cancelled["id"], 8);
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    assert_eq!(client.receive().await["id"], 9);
    // Cancelling a queued call must not cancel the active acquisition.
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        socket.read_to_end(&mut Vec::new())
    )
    .await
    .is_err());
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}))
        .await;
    let cancelled = client.receive().await;
    assert_eq!(cancelled["id"], 7);
    assert_eq!(cancelled["error"]["code"], -32800);
    for id in [json!("8"), json!(10)] {
        let response = client.receive().await;
        assert_eq!(response["id"], id);
        assert!(response["result"]["tools"].is_array(), "{response}");
    }
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    drop(client.input.take());
    assert!(client.stopped().await.success());
}

#[tokio::test]
async fn admission_bounds_reject_only_excess_and_duplicate_calls() {
    for wide in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = Client::start(&stalled(&listener)).await;
        client.path(json!(7)).await;
        let (_socket, _) = listener.accept().await.unwrap();
        let count = if wide { 4 } else { 8 };
        let request = |id| {
            json!({"jsonrpc":"2.0","id":id,"method":"tools/list",
            "params":{"padding":"x".repeat(if wide { 60_000 } else { 0 })}})
        };
        for id in 100..100 + count {
            client.send(request(id)).await;
        }
        client.send(request(200)).await;
        for id in [7, 100] {
            client.send(request(id)).await;
        }
        client
            .send(json!({"jsonrpc":"2.0","id":201,"method":"ping"}))
            .await;
        let rejected = client.receive().await;
        assert_eq!(rejected["id"], 200);
        assert_eq!(rejected["error"]["code"], -32000);
        assert_eq!(rejected["error"]["message"], "MCP admission queue full");
        for _ in 0..2 {
            let duplicate = client.receive().await;
            assert!(duplicate["id"].is_null(), "{duplicate}");
            assert_eq!(duplicate["error"]["code"], -32600);
        }
        assert_eq!(client.receive().await["id"], 201);
        client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":100}})).await;
        let cancelled = client.receive().await;
        assert_eq!(cancelled["id"], 100);
        assert_eq!(cancelled["error"]["code"], -32800);
        // Removing a queued frame returns both its count and byte capacity.
        client.send(request(300)).await;
        client
            .send(json!({"jsonrpc":"2.0","id":301,"method":"ping"}))
            .await;
        assert_eq!(client.receive().await["id"], 301);
        client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}})).await;
        assert_eq!(client.receive().await["error"]["code"], -32800);
        for id in (101..100 + count).chain([300]) {
            let response = client.receive().await;
            assert_eq!(response["id"], id);
            assert!(response["result"]["tools"].is_array(), "{response}");
        }
        drop(client.input.take());
        assert!(client.stopped().await.success());
    }
}

#[tokio::test]
async fn queued_wait_expires_without_starting_another_acquisition() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = Client::start(&stalled(&listener)).await;
    let status = |id| {
        json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
        "params":{"name":"status","arguments":{"repo":"demo"}}})
    };
    client.send(status(7)).await;
    let (mut socket, _) = listener.accept().await.unwrap();
    let started = std::time::Instant::now();
    client.send(status(8)).await;
    client
        .send(json!({"jsonrpc":"2.0","id":9,"method":"ping"}))
        .await;
    assert_eq!(client.receive().await["id"], 9);
    let expired = client.receive_within(Duration::from_secs(12)).await;
    assert_eq!(expired["id"], 8);
    assert_eq!(expired["error"]["code"], -32000);
    assert_eq!(
        expired["error"]["message"],
        "MCP admission wait deadline exceeded"
    );
    assert!(started.elapsed() >= Duration::from_secs(9));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}))
        .await;
    assert_eq!(client.receive().await["error"]["code"], -32800);
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    client
        .send(json!({"jsonrpc":"2.0","id":10,"method":"tools/list"}))
        .await;
    assert!(client.receive().await["result"]["tools"].is_array());
    drop(client.input.take());
    assert!(client.stopped().await.success());
}

#[tokio::test]
async fn oversized_unterminated_frame_stops_without_waiting_for_eof() {
    let mut client = Client::start(REFUSED).await;
    client
        .input
        .as_mut()
        .unwrap()
        .write_all(&vec![b' '; 65_537])
        .await
        .unwrap();
    assert!(!client.stopped().await.success());
}

#[tokio::test]
async fn eof_after_a_large_protocol_error_exits_cleanly() {
    let mut client = Client::start(REFUSED).await;
    // The error echoes this method. The required live wide lookup below
    // establishes backpressure even on hosts with larger pipe capacities.
    client
        .send(json!({"jsonrpc":"2.0","id":1,"method":"x".repeat(60_000)}))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(client.input.take());
    assert!(client.stopped().await.success());
}

/// Count the MCP's backend sessions; fixture stores connect under another name.
fn sessions(server: &server::Server, condition: &str) -> u64 {
    server
        .psql(
            "chaosbox",
            &format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'chaosbox' AND {condition}"),
        )
        .trim()
        .parse()
        .unwrap()
}

async fn wait_sessions(server: &server::Server, condition: &str, expected: u64) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while sessions(server, condition) != expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("MCP sessions where {condition} did not reach {expected}"));
}

/// Fingerprint of MCP statement activity: sessions, last state change, active.
fn activity(server: &server::Server) -> String {
    server.psql(
        "chaosbox",
        "SELECT count(*), max(state_change), count(*) FILTER (WHERE state = 'active') FROM pg_stat_activity WHERE application_name = 'chaosbox'",
    )
}

/// Holds an exclusive lock on `relationships` so a graph read stays in flight
/// until [`Blocker::release`]; EOF on psql's input rolls the lock back.
struct Blocker(std::process::Child);

impl Blocker {
    fn hold(server: &server::Server) -> Self {
        use std::io::Write as _;
        let psql = std::env::var_os("PG_BIN").map_or_else(
            || "psql".into(),
            |dir| std::path::Path::new(&dir).join("psql"),
        );
        let mut child = std::process::Command::new(psql)
            .args(["-XAtq", "--set", "ON_ERROR_STOP=1"])
            .arg(server.url())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"BEGIN;\nLOCK TABLE relationships IN ACCESS EXCLUSIVE MODE;\n")
            .unwrap();
        let blocker = Self(child);
        let granted = "SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid = l.relation WHERE c.relname = 'relationships' AND l.mode = 'AccessExclusiveLock' AND l.granted";
        let started = std::time::Instant::now();
        while server.psql("chaosbox", granted).trim() != "1" {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "lock not granted"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        blocker
    }

    fn release(&mut self) {
        if let Some(input) = self.0.stdin.take() {
            drop(input);
            let _ = self.0.wait();
        }
    }
}

impl Drop for Blocker {
    fn drop(&mut self) {
        self.release();
    }
}

/// Migrated fixture store with the shared snapshot registered. It connects as
/// `fixture`, so session counts observe only the MCP under test.
async fn fixture_store(server: &server::Server) -> chaosbox_pg::store::PgStore {
    use chaosbox_core::SnapshotFile;
    use chaosbox_store::Store;
    let config =
        chaosbox_pg::PgConfig::new(format!("{} application_name=fixture", server.url())).unwrap();
    let mut store = chaosbox_pg::store::PgStore::new(config);
    store.migrate().await.unwrap();
    store
        .ensure_snapshot_files(
            SNAP,
            REPO,
            &[SnapshotFile {
                snapshot: SNAP.into(),
                path: "f.rs".into(),
                sha256: "fixture".into(),
                bytes: 1,
            }],
        )
        .await
        .unwrap();
    store
}

fn sample(pid: u32) -> Value {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let mut values = serde_json::Map::new();
    for key in ["VmRSS", "RssAnon", "VmHWM", "VmSwap", "Threads"] {
        let line = status
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}:")))
            .unwrap();
        values.insert(
            key.into(),
            json!(line
                .split_whitespace()
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap()),
        );
    }
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .count();
    values.insert("fds".into(), json!(fds));
    Value::Object(values)
}

async fn call(client: &mut Client, id: u64, name: &str, arguments: Value) -> Value {
    client.send(json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":name,"arguments":arguments}})).await;
    let response = client.receive_within(Duration::from_secs(15)).await;
    assert_eq!(response["id"], id);
    response
}

fn payload(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

async fn check_cleanup(
    mut client: Client,
    mode: &str,
    server: &server::Server,
    path: Value,
) -> Value {
    // Prime successful graph reuse first: transport failures must also release
    // a reused connection, not only a freshly acquired request-owned session.
    payload(&call(&mut client, 100, "status", json!({"repo":REPO})).await);
    let mut blocker = Blocker::hold(server);
    client.send(json!({"jsonrpc":"2.0","id":101,"method":"tools/call","params":{"name":"path","arguments":path}})).await;
    wait_sessions(server, "wait_event_type = 'Lock'", 1).await;
    client
        .send(json!({"jsonrpc":"2.0","id":104,"method":"tools/list"}))
        .await;
    client
        .send(json!({"jsonrpc":"2.0","id":105,"method":"ping"}))
        .await;
    assert_eq!(client.receive().await["id"], 105);
    let started = std::time::Instant::now();
    match mode {
        "cancel" => {
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"101"}})).await;
            client
                .send(json!({"jsonrpc":"2.0","id":102,"method":"ping"}))
                .await;
            assert_eq!(client.receive().await["id"], 102);
            assert_eq!(
                sessions(server, "wait_event_type = 'Lock'"),
                1,
                "read still in flight"
            );
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":101}})).await;
            let response = client.receive().await;
            assert_eq!(response["id"], 101);
            assert_eq!(response["error"]["code"], -32800);
            let queued = client.receive().await;
            assert_eq!(queued["id"], 104);
            assert!(queued["result"]["tools"].is_array());
            // The cancelled request owned the reused session: nothing survives.
            blocker.release();
            wait_sessions(server, "true", 0).await;
            client
                .send(json!({"jsonrpc":"2.0","id":103,"method":"ping"}))
                .await;
            assert_eq!(client.receive().await["id"], 103);
            drop(client.input.take());
            assert!(client.stopped().await.success());
        }
        "eof" => {
            drop(client.input.take());
            assert!(client.stopped().await.success());
        }
        "oversize" => {
            client
                .input
                .as_mut()
                .unwrap()
                .write_all(&vec![b'x'; 65_537])
                .await
                .unwrap();
            assert!(!client.stopped().await.success());
        }
        "closed-output" => {
            let Client {
                mut child,
                mut input,
                output,
            } = client;
            drop(output);
            input
                .as_mut()
                .unwrap()
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":102,\"method\":\"ping\"}\n")
                .await
                .unwrap();
            assert!(!tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success());
        }
        _ => {
            let ping = b"{\"jsonrpc\":\"2.0\",\"id\":102,\"method\":\"ping\"}\n".repeat(4096);
            // A closed pipe while the flood is still being written is the
            // expected fail-closed overload behavior.
            let _ = client.input.as_mut().unwrap().write_all(&ping).await;
            assert!(!client.stopped().await.success());
        }
    }
    blocker.release();
    wait_sessions(server, "true", 0).await;
    json!({"mode":mode,"cleanup_ms":started.elapsed().as_millis()})
}

async fn check_wide(mut client: Client, mode: &str, server: &server::Server, id: &str) -> Value {
    // A wide real lookup guarantees pipe backpressure. Its reads must already
    // have finished while bytes remain unread, and EOF must not strand a
    // Tokio blocking stdout worker during runtime shutdown.
    client.send(json!({"jsonrpc":"2.0","id":201,"method":"tools/call","params":{"name":"lookup","arguments":{"repo":REPO,"id":id}}})).await;
    wait_sessions(
        server,
        "state = 'idle' AND query LIKE '%JOIN entities e ON%'",
        1,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(client.child.try_wait().unwrap().is_none());
    let started = std::time::Instant::now();
    if mode == "wide-eof" {
        drop(client.input.take());
        assert!(client.stopped().await.success());
    } else {
        let status = tokio::time::timeout(Duration::from_secs(12), client.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!status.success());
        assert!(started.elapsed() >= Duration::from_secs(8));
    }
    wait_sessions(server, "true", 0).await;
    json!({"mode":mode,"cleanup_ms":started.elapsed().as_millis()})
}

async fn check_retention(
    mut client: Client,
    server: &server::Server,
    hub: &str,
    isolated: &str,
) -> Vec<Value> {
    let path = |hops| json!({"repo":REPO,"from":hub,"to":isolated,"max_hops":hops});
    let mut samples = Vec::new();
    for batch in 0..7 {
        for index in 0..20 {
            let arguments = match index % 3 {
                0 => json!({"repo":REPO,"from":hub,"to":hub,"max_hops":0}),
                1 => json!({"repo":REPO,"from":hub,"to":"ent:missing","max_hops":1}),
                _ => path(1),
            };
            let response = call(&mut client, 1000 + batch * 20 + index, "path", arguments).await;
            assert!(response.get("error").is_none(), "{response}");
            let payload: Value =
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            if index % 3 == 0 {
                assert_eq!(payload["path"], json!([hub]));
            } else {
                assert!(payload["path"].is_null());
            }
        }
        let response = call(&mut client, 5000 + batch, "path", path(2)).await;
        assert_eq!(response["error"]["code"], -32603);
        let message = response["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("path traversal budget exceeded")
                || message.contains("path traversal deadline exceeded"),
            "{response}"
        );
        // A failed call never returns its session to the reuse slot.
        wait_sessions(server, "true", 0).await;
        let sample = sample(client.child.id().unwrap());
        eprintln!(
            "{}",
            json!({"case":"persistent-retention-sample", "batch":batch, "sample":sample})
        );
        samples.push(sample);
    }
    let warm = &samples[0];
    for observed in &samples[1..] {
        assert!(
            observed["VmRSS"].as_u64().unwrap() <= warm["VmRSS"].as_u64().unwrap() + 16 * 1024,
            "persistent RSS did not plateau: {samples:?}"
        );
        assert!(observed["Threads"].as_u64().unwrap() <= warm["Threads"].as_u64().unwrap() + 2);
        assert!(observed["fds"].as_u64().unwrap() <= warm["fds"].as_u64().unwrap() + 2);
        assert_eq!(observed["VmSwap"], 0);
    }
    drop(client.input.take());
    assert!(client.stopped().await.success());
    samples
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_ordinary_mcp_cleanup_and_retention() {
    use chaosbox_core::{
        Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SourceSpan,
    };
    use chaosbox_store::Store;
    let server = server::Server::start();
    let mut store = fixture_store(&server).await;
    let node = |name: &str| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            SNAP,
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = node("hub");
    let isolated = node("isolated");
    let large = node(&"wide".repeat(150_000));
    let mut build = GraphBuild::new(REPO, vec![SNAP.into()], 1);
    for node in [&hub, &isolated, &large] {
        build.add_node(node.clone()).unwrap();
    }
    for index in 0..650 {
        let leaf = node(&format!("leaf-{index}"));
        build.add_node(leaf.clone()).unwrap();
        build
            .add_edge(Relation::new(
                RelationType::Calls,
                &hub.id,
                &leaf.id,
                RelationScope::File,
                &build.id,
            ))
            .unwrap();
    }
    let build_id = build.id.clone();
    store.publish(build, None).await.unwrap();
    let url = server.url();
    let mut cleanup = Vec::new();
    for mode in [
        "cancel",
        "eof",
        "oversize",
        "closed-output",
        "output-overload",
    ] {
        let path = json!({"repo":REPO,"from":hub.id,"to":isolated.id,"max_hops":2});
        cleanup.push(check_cleanup(Client::start(&url).await, mode, &server, path).await);
    }
    for mode in ["wide-eof", "wide-deadline"] {
        cleanup.push(check_wide(Client::start(&url).await, mode, &server, &large.id).await);
    }
    let samples = check_retention(Client::start(&url).await, &server, &hub.id, &isolated.id).await;
    eprintln!(
        "{}",
        json!({"case":"ordinary-mcp-lifecycle-retention", "build":build_id,
        "nodes":653,"relationships":650,"wide_entity_name_bytes":large.name.len(),
        "cleanup":cleanup,"calls_in_one_persistent_process":147,"samples":samples})
    );
}

fn connection_fixture() -> (chaosbox_core::GraphBuild, String) {
    use chaosbox_core::{
        Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SourceSpan,
    };
    let mut build = GraphBuild::new(REPO, vec![SNAP.into()], 1);
    let node = |name| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            SNAP,
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = node("hub");
    build.add_node(hub.clone()).unwrap();
    for name in ["a", "b"] {
        let leaf = node(name);
        build.add_node(leaf.clone()).unwrap();
        build
            .add_edge(Relation::new(
                RelationType::Calls,
                &hub.id,
                &leaf.id,
                RelationScope::File,
                &build.id,
            ))
            .unwrap();
    }
    (build, hub.id)
}

/// Send a wide lookup and return once its reads finished while its response
/// is still blocked on unread output.
async fn hold_wide_response(
    client: &mut Client,
    server: &server::Server,
    id: u64,
    entity: &str,
) -> String {
    client
        .send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
        "params":{"name":"lookup","arguments":{"repo":REPO,"id":entity}}}))
        .await;
    wait_sessions(
        server,
        "state = 'idle' AND query LIKE '%JOIN entities e ON%'",
        1,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    activity(server)
}

fn status_request(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
        "params":{"name":"status","arguments":{"repo":REPO}}})
}

async fn queued_repin(
    client: &mut Client,
    store: &mut chaosbox_pg::store::PgStore,
    server: &server::Server,
    wide: &chaosbox_core::Entity,
    old: &str,
) -> String {
    use chaosbox_core::GraphBuild;
    use chaosbox_store::Store;
    let held = hold_wide_response(client, server, 1, &wide.id).await;
    for id in 10..18 {
        client.send(status_request(id)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        activity(server),
        held,
        "queued calls started behind blocked output"
    );
    let mut replacement = GraphBuild::new(REPO, vec![SNAP.into()], 2);
    replacement.add_node(wide.clone()).unwrap();
    let current = replacement.id.clone();
    store
        .publish(replacement, Some(old.to_owned()))
        .await
        .unwrap();
    assert_eq!(payload(&client.receive().await)["entity_id"], wide.id);
    // Queued calls pin at dispatch, so all observe the moved pointer.
    for id in 10..18 {
        let response = client.receive().await;
        assert_eq!(response["id"], id);
        assert_eq!(payload(&response)["build_id"], current);
    }
    current
}

async fn queued_status_retention(
    client: &mut Client,
    server: &server::Server,
    current: &str,
) -> Vec<Value> {
    let mut samples = Vec::new();
    for batch in 0..7 {
        let base = 1000 + batch * 8;
        for id in base..base + 8 {
            client.send(status_request(id)).await;
        }
        for id in base..base + 8 {
            let response = client.receive().await;
            assert_eq!(response["id"], id);
            assert_eq!(payload(&response)["build_id"], current);
        }
        // Successful calls reuse one session; bursts open no new connections.
        assert_eq!(sessions(server, "true"), 1);
        samples.push(sample(client.child.id().unwrap()));
    }
    for observed in &samples {
        assert_eq!(observed["VmSwap"], 0);
        assert!(
            observed["VmRSS"].as_u64().unwrap()
                <= samples[0]["VmRSS"].as_u64().unwrap() + 16 * 1024
        );
        assert!(
            observed["Threads"].as_u64().unwrap() <= samples[0]["Threads"].as_u64().unwrap() + 2
        );
        assert!(observed["fds"].as_u64().unwrap() <= samples[0]["fds"].as_u64().unwrap() + 2);
    }
    samples
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_queued_calls_repin_and_retain_bounded_state() {
    use chaosbox_core::{Entity, EntityKind, SourceSpan};
    use chaosbox_store::Store;
    let server = server::Server::start();
    let mut store = fixture_store(&server).await;
    let (mut build, _) = connection_fixture();
    let wide = Entity::new(
        EntityKind::Symbol,
        REPO,
        SNAP,
        "f.rs",
        &"wide".repeat(150_000),
        "wide",
        SourceSpan::point("f.rs", 1, 1, 0),
    );
    build.add_node(wide.clone()).unwrap();
    let old = build.id.clone();
    store.publish(build, None).await.unwrap();
    let mut client = Client::start(&server.url()).await;
    let current = queued_repin(&mut client, &mut store, &server, &wide, &old).await;
    let samples = queued_status_retention(&mut client, &server, &current).await;
    drop(client.input.take());
    assert!(client.stopped().await.success());
    wait_sessions(&server, "true", 0).await;
    eprintln!(
        "{}",
        json!({"case":"bounded-ordinary-admission", "old":old,"current":current,
        "queued_after_pointer_movement":8,"retention_bursts":7,"calls_per_burst":8,
        "samples":samples})
    );
}
