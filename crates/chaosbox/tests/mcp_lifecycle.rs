//! Ordinary stdio MCP must observe disconnects and cancellation during acquisition.

use std::{process::Stdio, time::Duration};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};

const REPO: &str = "mcp-lifecycle";
const SNAP: &str = "s-mcp-lifecycle";

struct Client {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: Lines<BufReader<tokio::process::ChildStdout>>,
}

impl Client {
    async fn start(address: &str, password: &std::path::Path) -> Self {
        Self::start_db(address, password, "s02c3_lifecycle").await
    }

    async fn start_db(address: &str, password: &std::path::Path, database: &str) -> Self {
        let binary = std::env::var_os("CHAOSBOX_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_chaosbox").into());
        Self::start_binary(address, password, database, binary).await
    }

    async fn start_binary(
        address: &str,
        password: &std::path::Path,
        database: &str,
        binary: std::ffi::OsString,
    ) -> Self {
        let mut child = tokio::process::Command::new(binary)
            .arg("mcp")
            .env("CHAOSBOX_TYPEDB_ADDR", address)
            .env("CHAOSBOX_TYPEDB_DATABASE", database)
            .env("CHAOSBOX_TYPEDB_PASSWORD_FILE", password)
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
    let credentials = tempfile::tempdir().unwrap();
    let password = credentials.path().join("password");
    std::fs::write(&password, "disposable-test-password").unwrap();
    let mut client = Client::start(&listener.local_addr().unwrap().to_string(), &password).await;
    client.path(json!(7)).await;
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
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
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut bytes))
        .await
        .expect("cancelled acquisition socket stayed open")
        .unwrap();
    assert!(
        !bytes.is_empty(),
        "real connector never wrote its HTTP/2 preface"
    );
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
async fn oversized_unterminated_frame_stops_without_waiting_for_eof() {
    let credentials = tempfile::tempdir().unwrap();
    let mut client = Client::start("127.0.0.1:1", &credentials.path().join("absent")).await;
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
    let credentials = tempfile::tempdir().unwrap();
    let mut client = Client::start("127.0.0.1:1", &credentials.path().join("absent")).await;
    // The error echoes this method. The required live wide lookup below
    // establishes backpressure even on hosts with larger pipe capacities.
    client
        .send(json!({"jsonrpc":"2.0","id":1,"method":"x".repeat(60_000)}))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(client.input.take());
    assert!(client.stopped().await.success());
}

async fn diagnostics() -> Value {
    let address = std::env::var("CHAOSBOX_TEST_MONITOR_ADDR")
        .expect("disposable monitoring address required");
    let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
    stream.write_all(format!("GET /diagnostics?format=json HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let body = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    serde_json::from_slice(&bytes[body..]).unwrap()
}

fn active_reads(metrics: &Value, database: &str) -> u64 {
    let load = metrics["load"]
        .as_array()
        .unwrap()
        .iter()
        .find(|load| load["database"] == database)
        .expect("disposable database missing in diagnostics");
    load["activeTransactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tx| tx["kind"] == "read" && tx["client"] == "grpc")
        .map(|tx| tx["count"].as_u64().unwrap())
        .sum()
}

async fn wait_reads(database: &str, active: bool) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let metrics = diagnostics().await;
            if (active_reads(&metrics, database) > 0) == active {
                return metrics;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("remote read did not reach the required state within two seconds")
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
    let fds: Vec<_> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .collect();
    values.insert("fds".into(), json!(fds.len()));
    let sockets: Vec<_> = fds
        .iter()
        .filter_map(|fd| {
            fd.as_ref()
                .ok()
                .and_then(|fd| std::fs::read_link(fd.path()).ok())
                .map(|path| path.to_string_lossy().into_owned())
                .filter(|path| path.starts_with("socket:["))
        })
        .collect();
    // Tokio's signal driver owns a Unix socket pair even before acquisition.
    // Match process-owned inodes to the kernel's TCP tables, not all sockets.
    let mut tcp_inodes = std::collections::BTreeSet::new();
    for table in ["tcp", "tcp6"] {
        for line in std::fs::read_to_string(format!("/proc/{pid}/net/{table}"))
            .unwrap()
            .lines()
            .skip(1)
        {
            if let Some(inode) = line.split_whitespace().nth(9) {
                tcp_inodes.insert(format!("socket:[{inode}]"));
            }
        }
    }
    values.insert(
        "tcp_sockets".into(),
        json!(sockets.iter().filter(|s| tcp_inodes.contains(*s)).count()),
    );
    values.insert("sockets".into(), json!(sockets.len()));
    Value::Object(values)
}

async fn settled_sample(pid: u32) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let sample = sample(pid);
            if sample["tcp_sockets"] == 0 {
                return sample;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ordinary MCP retained a backend connection")
}

async fn call(client: &mut Client, id: u64, name: &str, arguments: Value) -> Value {
    client.send(json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":name,"arguments":arguments}})).await;
    let response = client.receive_within(Duration::from_secs(15)).await;
    assert_eq!(response["id"], id);
    response
}

async fn check_cleanup(mut client: Client, mode: &str, database: &str, path: Value) -> Value {
    // Prime successful graph reuse first: transport failures must also release
    // a reused connection, not only a freshly acquired request-owned driver.
    payload(&call(&mut client, 100, "status", json!({"repo":REPO})).await);
    client.send(json!({"jsonrpc":"2.0","id":101,"method":"tools/call","params":{"name":"path","arguments":path}})).await;
    let active = wait_reads(database, true).await;
    let started = std::time::Instant::now();
    match mode {
        "cancel" => {
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"101"}})).await;
            client
                .send(json!({"jsonrpc":"2.0","id":102,"method":"ping"}))
                .await;
            assert_eq!(client.receive().await["id"], 102);
            wait_reads(database, true).await;
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":101}})).await;
            let response = client.receive().await;
            assert_eq!(response["id"], 101);
            assert_eq!(response["error"]["code"], -32800);
            wait_reads(database, false).await;
            let observed = settled_sample(client.child.id().unwrap()).await;
            assert_eq!(observed["tcp_sockets"], 0);
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
    let idle = wait_reads(database, false).await;
    json!({"mode":mode,"observed_active_reads":active_reads(&active,database),
            "remaining_reads":active_reads(&idle,database),"cleanup_ms":started.elapsed().as_millis()})
}

async fn check_wide(mut client: Client, mode: &str, database: &str, id: &str) -> Value {
    // A wide real lookup guarantees pipe backpressure. Its read transaction must
    // already have closed while bytes remain unread, and EOF must not strand a
    // Tokio blocking stdout worker during runtime shutdown.
    let query_count = |m: &Value| {
        m["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["database"] == database && a["name"] == "TRANSACTION_QUERY")
            .map_or(0, |a| a["successful"].as_u64().unwrap())
    };
    let before = query_count(&diagnostics().await);
    client.send(json!({"jsonrpc":"2.0","id":201,"method":"tools/call","params":{"name":"lookup","arguments":{"repo":REPO,"id":id}}})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let metrics = diagnostics().await;
            if query_count(&metrics) >= before + 3 && active_reads(&metrics, database) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
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
    json!({"mode":mode,"remaining_reads":active_reads(&diagnostics().await,database),"cleanup_ms":started.elapsed().as_millis()})
}

async fn check_retention(
    mut client: Client,
    database: &str,
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
        wait_reads(database, false).await;
        let sample = settled_sample(client.child.id().unwrap()).await;
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
#[ignore = "requires a fresh disposable s02c3_* TypeDB database, monitoring and CHAOSBOX_REQUIRE_TYPEDB=1"]
async fn disposable_ordinary_mcp_cleanup_and_retention() {
    use chaosbox_core::{
        Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SnapshotFile,
        SourceSpan,
    };
    use chaosbox_store::Store;
    use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};
    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    assert!(config.database.starts_with("s02c3_"));
    let mut store = TypeDbStore::new(config.clone());
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
    let password =
        std::path::PathBuf::from(std::env::var_os("CHAOSBOX_TYPEDB_PASSWORD_FILE").unwrap());
    let new_client = || Client::start_db(&config.address, &password, &config.database);
    let before = wait_reads(&config.database, false).await;
    let mut cleanup = Vec::new();
    for mode in [
        "cancel",
        "eof",
        "oversize",
        "closed-output",
        "output-overload",
    ] {
        let path = json!({"repo":REPO,"from":hub.id,"to":isolated.id,"max_hops":2});
        cleanup.push(check_cleanup(new_client().await, mode, &config.database, path).await);
    }
    for mode in ["wide-eof", "wide-deadline"] {
        cleanup.push(check_wide(new_client().await, mode, &config.database, &large.id).await);
    }
    let samples =
        check_retention(new_client().await, &config.database, &hub.id, &isolated.id).await;
    let after = wait_reads(&config.database, false).await;
    eprintln!(
        "{}",
        json!({"case":"ordinary-mcp-lifecycle-retention", "build":build_id,
        "nodes":653,"relationships":650,"wide_entity_name_bytes":large.name.len(),
        "cleanup":cleanup,"calls_in_one_persistent_process":147,"warmup_calls":21,"samples":samples,
        "diagnostics_before":before,"diagnostics_after":after})
    );
}

fn action_count(metrics: &Value, database: &str, action: &str) -> u64 {
    metrics["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["name"] == action && (action == "CONNECTION_OPEN" || row["database"] == database)
        })
        .map(|row| row["successful"].as_u64().unwrap())
        .sum()
}

fn payload(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

async fn lookup_batch(client: &mut Client, id: &str, database: &str, base_id: u64) -> Value {
    let before = diagnostics().await;
    for index in 0..20 {
        let row = payload(
            &call(
                client,
                base_id + index,
                "lookup",
                json!({"repo":REPO,"id":id}),
            )
            .await,
        );
        assert_eq!(row["entity_id"], id);
    }
    let after = wait_reads(database, false).await;
    json!({"connections":action_count(&after,database,"CONNECTION_OPEN") - action_count(&before,database,"CONNECTION_OPEN"),
        "transactions":action_count(&after,database,"TRANSACTION_OPEN") - action_count(&before,database,"TRANSACTION_OPEN"),
        "queries":action_count(&after,database,"TRANSACTION_QUERY") - action_count(&before,database,"TRANSACTION_QUERY"),
        "sample":sample(client.child.id().unwrap())})
}

async fn repin_and_rotate(
    client: &mut Client,
    store: &mut chaosbox_typedb::store::TypeDbStore,
    password: &std::path::Path,
    config: &chaosbox_typedb::store::TypeDbConfig,
    hub: &str,
    old: &str,
) {
    use chaosbox_store::Store;
    let page = payload(
        &call(
            client,
            10000,
            "neighbors",
            json!({"repo":REPO,"id":hub,"limit":1,"direction":"outgoing"}),
        )
        .await,
    );
    let cursor = page["next_cursor"]
        .as_str()
        .expect("old publication needs a second page");
    let replacement = chaosbox_core::GraphBuild::new(REPO, vec![SNAP.into()], 2);
    let id = replacement.id.clone();
    // This publication deliberately has no members. Current lookup must not
    // borrow the previous request's pin; continuation still selects the old one.
    store
        .publish(replacement, Some(old.to_owned()))
        .await
        .unwrap();
    assert!(payload(&call(client, 10001, "lookup", json!({"repo":REPO,"id":hub})).await).is_null());
    let status = payload(&call(client, 10002, "status", json!({"repo":REPO})).await);
    assert_eq!(status["build_id"], id);
    let historical = payload(
        &call(
            client,
            10003,
            "neighbors",
            json!({"repo":REPO,"id":hub,"limit":1,"direction":"outgoing","cursor":cursor}),
        )
        .await,
    );
    assert_eq!(historical["build_id"], old);
    let other = "s04a1-other";
    store
        .publish(chaosbox_core::GraphBuild::new(other, vec![], 1), None)
        .await
        .unwrap();
    assert!(
        payload(&call(client, 10004, "lookup", json!({"repo":other,"id":hub})).await).is_null()
    );
    assert!(call(
        client,
        10005,
        "neighbors",
        json!({"repo":other,"id":hub,"limit":1,"direction":"outgoing","cursor":cursor})
    )
    .await
    .get("error")
    .is_some());
    payload(&call(client, 10006, "status", json!({"repo":REPO})).await);
    std::fs::write(password, "disposable-wrong-password").unwrap();
    assert!(call(client, 10007, "status", json!({"repo":REPO}))
        .await
        .get("error")
        .is_some());
    assert_eq!(
        settled_sample(client.child.id().unwrap()).await["tcp_sockets"],
        0
    );
    std::fs::write(password, &config.password).unwrap();
    assert_eq!(
        payload(&call(client, 10008, "status", json!({"repo":REPO})).await)["build_id"],
        id
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

#[tokio::test]
#[ignore = "requires fresh disposable s04a1_* TypeDB, monitoring, CHAOSBOX_BASELINE_TEST_BIN and CHAOSBOX_REQUIRE_TYPEDB=1"]
async fn disposable_ordinary_mcp_connection_reuse_is_pinned_and_bounded() {
    use chaosbox_core::SnapshotFile;
    use chaosbox_store::Store;
    use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};
    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    assert!(config.database.starts_with("s04a1_"));
    let mut store = TypeDbStore::new(config.clone());
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
    let (build, hub) = connection_fixture();
    let old = build.id.clone();
    store.publish(build, None).await.unwrap();
    let credentials = tempfile::tempdir().unwrap();
    let password = credentials.path().join("password");
    std::fs::write(&password, &config.password).unwrap();
    let baseline_binary = std::env::var_os("CHAOSBOX_BASELINE_TEST_BIN").unwrap();
    let mut baseline = Client::start_binary(
        &config.address,
        &password,
        &config.database,
        baseline_binary,
    )
    .await;
    let before = lookup_batch(&mut baseline, &hub, &config.database, 1).await;
    drop(baseline.input.take());
    assert!(baseline.stopped().await.success());
    let mut client = Client::start_db(&config.address, &password, &config.database).await;
    let mut batches = Vec::new();
    for batch in 0..7 {
        batches.push(lookup_batch(&mut client, &hub, &config.database, 100 + batch * 20).await);
        if batch == 0 {
            eprintln!(
                "{}",
                json!({"case":"connection-reuse-before-after","baseline":before,"candidate":batches[0]})
            );
            assert!(
                batches[0]["connections"].as_u64().unwrap()
                    < before["connections"].as_u64().unwrap()
            );
            assert_eq!(batches[0]["queries"], before["queries"]);
        }
        let observed = batches.last().unwrap();
        assert_eq!(observed["connections"], u64::from(batch == 0));
        assert_eq!(observed["queries"], 60);
        let current = &observed["sample"];
        assert_eq!(current["tcp_sockets"], 1);
        assert_eq!(current["VmSwap"], 0);
        let warm = &batches[0]["sample"];
        assert!(current["VmRSS"].as_u64().unwrap() <= warm["VmRSS"].as_u64().unwrap() + 16 * 1024);
        assert!(current["Threads"].as_u64().unwrap() <= warm["Threads"].as_u64().unwrap() + 2);
        assert!(current["fds"].as_u64().unwrap() <= warm["fds"].as_u64().unwrap() + 2);
    }
    repin_and_rotate(&mut client, &mut store, &password, &config, &hub, &old).await;
    let started = std::time::Instant::now();
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert_eq!(sample(client.child.id().unwrap())["tcp_sockets"], 1);
    client
        .send(json!({"jsonrpc":"2.0","id":19999,"method":"ping"}))
        .await;
    assert_eq!(client.receive().await["id"], 19999);
    tokio::time::sleep(Duration::from_secs(15)).await;
    let idle = sample(client.child.id().unwrap());
    assert_eq!(
        idle["tcp_sockets"], 0,
        "idle connection was retained: {idle}"
    );
    payload(&call(&mut client, 20000, "status", json!({"repo":REPO})).await);
    drop(client.input.take());
    assert!(client.stopped().await.success());
    wait_reads(&config.database, false).await;
    eprintln!(
        "{}",
        json!({"case":"ordinary-connection-reuse","baseline_calls":20,"baseline":before,
        "candidate_success_calls":140,"batches":batches,"idle":idle,"idle_elapsed_ms":started.elapsed().as_millis(),
        "pointer_repo_credential_and_historical_checks":true})
    );
}

async fn header_cases(
    client: &mut Client,
    baseline: &mut Client,
    store: &mut chaosbox_typedb::store::TypeDbStore,
    database: &str,
) {
    use chaosbox_store::Store;
    for snapshots in [vec![], vec!["s04a2-header".to_owned()]] {
        let repo = format!("s04a2-snapshots-{}", snapshots.len());
        store
            .publish(
                chaosbox_core::GraphBuild::new(&repo, snapshots.clone(), 1),
                None,
            )
            .await
            .unwrap();
        let expected = payload(&call(baseline, 500, "status", json!({"repo":repo})).await);
        let before = diagnostics().await;
        let actual = payload(&call(client, 500, "status", json!({"repo":repo})).await);
        let after = wait_reads(database, false).await;
        assert_eq!(
            action_count(&after, database, "TRANSACTION_OPEN")
                - action_count(&before, database, "TRANSACTION_OPEN"),
            1
        );
        assert_eq!(
            action_count(&after, database, "TRANSACTION_QUERY")
                - action_count(&before, database, "TRANSACTION_QUERY"),
            2
        );
        assert_eq!(actual, expected);
        assert_eq!(actual["snapshots"], json!(snapshots));
    }
}

async fn historical_header_batch(config: &chaosbox_typedb::store::TypeDbConfig, old: &str) {
    use chaosbox_store::GraphQueries;
    let mut reader = chaosbox_typedb::reader::TypeDbReader::new(config.clone());
    reader.connect().await.unwrap();
    let before = diagnostics().await;
    let historical = reader.published_build(REPO, old).await.unwrap().unwrap();
    assert_eq!(historical.build_id, old);
    assert_eq!(historical.generation, 1);
    assert_eq!(historical.snapshots, [SNAP]);
    assert!(historical.evidence_sealed);
    assert!(reader
        .published_build("s04a2-foreign", old)
        .await
        .unwrap()
        .is_none());
    assert!(reader
        .published_build(REPO, "build:missing")
        .await
        .unwrap()
        .is_none());
    let after = wait_reads(&config.database, false).await;
    assert_eq!(
        action_count(&after, &config.database, "TRANSACTION_OPEN")
            - action_count(&before, &config.database, "TRANSACTION_OPEN"),
        3
    );
    assert_eq!(
        action_count(&after, &config.database, "TRANSACTION_QUERY")
            - action_count(&before, &config.database, "TRANSACTION_QUERY"),
        4
    );
}

#[tokio::test]
#[ignore = "requires fresh disposable s04a2_* TypeDB, monitoring, S04a.1 CHAOSBOX_BASELINE_TEST_BIN and CHAOSBOX_REQUIRE_TYPEDB=1"]
async fn disposable_ordinary_mcp_header_batches_reduce_transactions() {
    use chaosbox_core::SnapshotFile;
    use chaosbox_store::Store;
    use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};
    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    assert!(config.database.starts_with("s04a2_"));
    let mut store = TypeDbStore::new(config.clone());
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
    let (build, hub) = connection_fixture();
    let old = build.id.clone();
    store.publish(build, None).await.unwrap();
    let credentials = tempfile::tempdir().unwrap();
    let password = credentials.path().join("password");
    std::fs::write(&password, &config.password).unwrap();
    let mut baseline = Client::start_binary(
        &config.address,
        &password,
        &config.database,
        std::env::var_os("CHAOSBOX_BASELINE_TEST_BIN").unwrap(),
    )
    .await;
    let before = lookup_batch(&mut baseline, &hub, &config.database, 1).await;
    let mut client = Client::start_db(&config.address, &password, &config.database).await;
    let after = lookup_batch(&mut client, &hub, &config.database, 1).await;
    eprintln!(
        "{}",
        json!({"case":"header-batch-before-after", "lookups_each":20, "baseline":before, "candidate":after})
    );
    assert_eq!(before["transactions"], 60);
    assert_eq!(after["transactions"], 40);
    assert_eq!(before["queries"], 60);
    assert_eq!(after["queries"], 60);
    assert_eq!(before["connections"], 1);
    assert_eq!(after["connections"], 1);
    header_cases(&mut client, &mut baseline, &mut store, &config.database).await;
    repin_and_rotate(&mut client, &mut store, &password, &config, &hub, &old).await;
    historical_header_batch(&config, &old).await;
    let before_missing = diagnostics().await;
    assert!(call(
        &mut client,
        20000,
        "status",
        json!({"repo":"s04a2-missing"})
    )
    .await
    .get("error")
    .is_some());
    let missing = wait_reads(&config.database, false).await;
    assert_eq!(
        action_count(&missing, &config.database, "TRANSACTION_OPEN")
            - action_count(&before_missing, &config.database, "TRANSACTION_OPEN"),
        1
    );
    assert_eq!(
        action_count(&missing, &config.database, "TRANSACTION_QUERY")
            - action_count(&before_missing, &config.database, "TRANSACTION_QUERY"),
        1
    );
    assert_eq!(
        settled_sample(client.child.id().unwrap()).await["tcp_sockets"],
        0
    );
    drop(client.input.take());
    drop(baseline.input.take());
    assert!(client.stopped().await.success());
    assert!(baseline.stopped().await.success());
    eprintln!(
        "{}",
        json!({"case":"header-batch-pin-parity", "zero_and_single_snapshot":true,
        "historical_repository_and_credential_checks":true, "missing_header_queries":1, "missing_header_transactions":1,
        "remaining_reads":active_reads(&wait_reads(&config.database, false).await, &config.database)})
    );
}
