//! Ordinary path clients validate before credentials and share traversal results.

#[path = "../../chaosbox-pg/tests/support/server.rs"]
mod server;

use std::{process::Stdio, time::Duration};
use chaosbox::{GraphReader, PathQuery};
use chaosbox_core::{
    Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SnapshotFile, SourceSpan,
};
use chaosbox_store::Store;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

fn binary() -> std::ffi::OsString {
    std::env::var_os("CHAOSBOX_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_chaosbox").into())
}

#[tokio::test]
async fn cli_validates_paths_before_credentials() {
    let cases: [(String, String, String, &str); 6] = [
        ("demo".into(), "ent:a".into(), "ent:b".into(), "9"),
        (" ".into(), "ent:a".into(), "ent:b".into(), "4"),
        ("x".repeat(257), "ent:a".into(), "ent:b".into(), "4"),
        ("demo".into(), " ".into(), "ent:b".into(), "4"),
        ("demo".into(), "ent:a".into(), "ent:\ncontrol".into(), "4"),
        ("demo".into(), "é".repeat(129), "ent:b".into(), "4"),
    ];
    for (repo, from, to, hops) in cases {
        let output = tokio::process::Command::new(binary())
            .args([
                "query",
                "path",
                &from,
                &to,
                "--repo",
                &repo,
                "--max-hops",
                hops,
            ])
            .env_remove("CHAOSBOX_DATABASE_URL")
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("invalid path"), "{text}");
        assert!(!text.contains("CHAOSBOX_DATABASE_URL"), "{text}");
    }
}

struct Mcp {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
}

impl Mcp {
    async fn start() -> Self {
        Self::start_command(tokio::process::Command::new(binary())).await
    }

    async fn start_command(mut command: tokio::process::Command) -> Self {
        let mut child = command
            .arg("mcp")
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Self {
            child,
            input,
            output,
            id: 0,
        };
        let response = client
            .rpc("initialize", json!({"protocolVersion":"2025-06-18"}))
            .await;
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        client
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let request = json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params});
        tokio::time::timeout(Duration::from_secs(30), async {
            self.input
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            self.input.flush().await.unwrap();
            let line = self
                .output
                .next_line()
                .await
                .unwrap()
                .expect("MCP closed early");
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], self.id);
            response
        })
        .await
        .expect("MCP response exceeded 30 seconds")
    }

    async fn path(&mut self, args: &Value) -> Result<Value, String> {
        let response = self
            .rpc("tools/call", json!({"name":"path","arguments":args}))
            .await;
        if let Some(error) = response.get("error") {
            assert_eq!(error["code"], -32603, "{response}");
            Err(error["message"].as_str().unwrap().into())
        } else {
            Ok(
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
            )
        }
    }

    async fn stop(self) {
        let Self {
            mut child, input, ..
        } = self;
        drop(input);
        assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    }
}

async fn cli_path(args: &Value) -> Result<Value, String> {
    let mut command = tokio::process::Command::new(binary());
    command.args([
        "query",
        "path",
        args["from"].as_str().unwrap(),
        args["to"].as_str().unwrap(),
        "--repo",
        args["repo"].as_str().unwrap(),
    ]);
    if let Some(hops) = args.get("max_hops") {
        command.args(["--max-hops", &hops.to_string()]);
    }
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        command
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    if output.status.success() {
        Ok(value)
    } else {
        Err(value["detail"]["reason"].as_str().unwrap().into())
    }
}

#[tokio::test]
async fn ordinary_path_deadline_includes_a_stalled_connection() {
    // A TCP listener that never answers cannot complete PostgreSQL startup.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "host=127.0.0.1 port={} dbname=stalled user=stalled",
        listener.local_addr().unwrap().port()
    );
    let command = || {
        let mut command = tokio::process::Command::new(binary());
        command
            .env("CHAOSBOX_DATABASE_URL", &url)
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE");
        command
    };
    let cli = async {
        let start = std::time::Instant::now();
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            command()
                .args(["query", "path", "ent:a", "ent:b", "--repo", "demo"])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            value["detail"]["reason"]
                .as_str()
                .unwrap()
                .contains("path traversal deadline exceeded"),
            "{value}"
        );
        start.elapsed().as_millis()
    };
    let mcp = async {
        let mut client = Mcp::start_command(command()).await;
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            client.path(&json!({"repo":"demo","from":"ent:a","to":"ent:b"})),
        )
        .await
        .unwrap();
        assert!(result
            .unwrap_err()
            .contains("path traversal deadline exceeded"));
        let elapsed = start.elapsed().as_millis();
        assert!(client.rpc("ping", json!({})).await.get("error").is_none());
        client.stop().await;
        elapsed
    };
    let (cli_ms, mcp_ms) = tokio::join!(cli, mcp);
    eprintln!(
        "{}",
        json!({"case":"stalled-acquisition-deadline","fresh_cli_ms":cli_ms,"persistent_mcp_ms":mcp_ms})
    );
}

async fn identical_paths(client: &mut Mcp, args: &Value) -> Result<Value, String> {
    let result = cli_path(args).await;
    let mut fresh = Mcp::start().await;
    let fresh_result = fresh.path(args).await;
    fresh.stop().await;
    let repeated = client.path(args).await;
    // Either whole-request ceiling may finish a slow exhausted search first.
    // Successful values are compared exactly; exhaustion must never become null.
    if result.is_err() {
        for outcome in [&result, &fresh_result, &repeated] {
            let error = outcome.as_ref().unwrap_err();
            assert!(
                error.contains("path traversal budget exceeded")
                    || error.contains("path traversal deadline exceeded"),
                "{error}"
            );
        }
        eprintln!(
            "{}",
            json!({"case":"exhaustion-by-transport", "fresh_cli":result, "fresh_mcp":fresh_result, "persistent_mcp":repeated})
        );
    } else {
        assert_eq!(fresh_result, result, "fresh MCP differs from CLI");
        assert_eq!(repeated, result, "persistent MCP differs from CLI");
    }
    result
}

const REPO: &str = "path-clients";
const DEGREE: usize = 650;

struct Fixture {
    build: GraphBuild,
    hub: String,
    late_out: String,
    late_in: String,
    isolated: String,
    foreign: Entity,
}

fn fixture() -> Fixture {
    let node = |name: &str| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            "s-path-clients",
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = node("hub");
    let isolated = node("isolated");
    let mut build = GraphBuild::new(REPO, vec!["s-path-clients".into()], 1);
    build.add_node(hub.clone()).unwrap();
    build.add_node(isolated.clone()).unwrap();
    let mut leaves = Vec::new();
    for index in 0..DEGREE {
        let leaf = node(&format!("leaf-{index}"));
        build.add_node(leaf.clone()).unwrap();
        let (from, to) = if index < 225 {
            (&hub.id, &leaf.id)
        } else {
            (&leaf.id, &hub.id)
        };
        edge(&mut build, RelationType::Calls, from, to);
        leaves.push(leaf.id);
    }
    edge(&mut build, RelationType::Calls, &hub.id, &hub.id);
    edge(&mut build, RelationType::References, &hub.id, &hub.id);
    edge(&mut build, RelationType::Calls, &leaves[0], &leaves[1]);
    edge(&mut build, RelationType::Calls, &leaves[1], &leaves[0]);
    let late_out = build
        .edges
        .values()
        .rfind(|edge| edge.from == hub.id && edge.to != hub.id)
        .unwrap()
        .to
        .clone();
    let late_in = build
        .edges
        .values()
        .rfind(|edge| edge.to == hub.id && edge.from != hub.id)
        .unwrap()
        .from
        .clone();
    Fixture {
        build,
        hub: hub.id,
        late_out,
        late_in,
        isolated: isolated.id,
        foreign: Entity::new(
            EntityKind::Symbol,
            "path-other",
            "s-path-other",
            "f.rs",
            "foreign",
            "foreign",
            SourceSpan::point("f.rs", 1, 1, 0),
        ),
    }
}

fn edge(build: &mut GraphBuild, kind: RelationType, from: &str, to: &str) {
    build
        .add_edge(Relation::new(
            kind,
            from,
            to,
            RelationScope::File,
            &build.id,
        ))
        .unwrap();
}

async fn check_cases(client: &mut Mcp, f: &Fixture) -> usize {
    let cases = [
        (&f.hub, &f.late_out, 1, json!([f.hub, f.late_out])),
        (&f.hub, &f.late_in, 1, json!([f.hub, f.late_in])),
        (&f.late_in, &f.hub, 1, json!([f.late_in, f.hub])),
        (
            &f.late_out,
            &f.late_in,
            2,
            json!([f.late_out, f.hub, f.late_in]),
        ),
        (&f.late_out, &f.late_in, 1, Value::Null),
        (&f.hub, &f.isolated, 1, Value::Null),
        (&f.hub, &f.late_out, 0, Value::Null),
        (&f.hub, &f.hub, 0, json!([f.hub])),
        (&f.hub, &f.hub, 8, json!([f.hub])),
    ];
    for (from, to, hops, path) in &cases {
        let args = json!({"repo":REPO,"from":from,"to":to,"max_hops":hops});
        assert_eq!(
            identical_paths(client, &args).await.unwrap(),
            json!({"from":from,"to":to,"path":path})
        );
    }
    for (from, to) in [
        ("ent:missing", "ent:missing"),
        (&f.hub, f.foreign.id.as_str()),
    ] {
        let args = json!({"repo":REPO,"from":from,"to":to,"max_hops":0});
        assert_eq!(
            identical_paths(client, &args).await.unwrap()["path"],
            Value::Null
        );
    }
    let default = json!({"repo":REPO,"from":f.hub,"to":f.late_out});
    assert_eq!(
        identical_paths(client, &default).await.unwrap()["path"],
        json!([f.hub, f.late_out])
    );
    let exhausted = json!({"repo":REPO,"from":f.hub,"to":f.isolated,"max_hops":2});
    let error = identical_paths(client, &exhausted).await.unwrap_err();
    eprintln!(
        "{}",
        json!({"case":"ordinary-path-exhaustion","reason":error})
    );
    cases.len() + 4
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_path_clients_agree_and_repin_after_publication() {
    use chaosbox_pg::{reader::PgReader, store::PgStore, PgConfig};

    let server = server::Server::start();
    // Fresh CLI and MCP children inherit the disposable database.
    std::env::set_var("CHAOSBOX_DATABASE_URL", server.url());
    let config = PgConfig::new(server.url()).unwrap();
    let mut store = PgStore::new(config.clone());
    store.migrate().await.unwrap();
    for (snapshot, repo) in [("s-path-clients", REPO), ("s-path-other", "path-other")] {
        store
            .ensure_snapshot_files(
                snapshot,
                repo,
                &[SnapshotFile {
                    snapshot: snapshot.into(),
                    path: "f.rs".into(),
                    sha256: "fixture".into(),
                    bytes: 1,
                }],
            )
            .await
            .unwrap();
    }
    let f = fixture();
    store.publish(f.build.clone(), None).await.unwrap();
    let mut other = GraphBuild::new("path-other", vec!["s-path-other".into()], 1);
    other.add_node(f.foreign.clone()).unwrap();
    store.publish(other, None).await.unwrap();
    let mut handle = PgReader::new(config.clone());
    handle.connect().await.unwrap();
    let old = GraphReader::pinned(handle, REPO).await.unwrap();
    let mut client = Mcp::start().await;
    let cases = check_cases(&mut client, &f).await;

    let mut new = GraphBuild::new(REPO, vec!["s-path-clients".into()], 2);
    new.predecessor = Some(f.build.id.clone());
    for id in [&f.hub, &f.late_out] {
        new.add_node(f.build.nodes[id].clone()).unwrap();
    }
    store
        .publish(new.clone(), new.predecessor.clone())
        .await
        .unwrap();
    let args = json!({"repo":REPO,"from":f.hub,"to":f.late_out,"max_hops":1});
    assert_eq!(
        identical_paths(&mut client, &args).await.unwrap()["path"],
        Value::Null
    );
    assert_eq!(
        old.path(&f.hub, &f.late_out, 1).await.unwrap(),
        Some(vec![f.hub.clone(), f.late_out.clone()])
    );
    let mut handle = PgReader::new(config.clone());
    handle.connect().await.unwrap();
    let historical = GraphReader::published(handle, REPO, &f.build.id)
        .await
        .unwrap();
    assert_eq!(
        historical.path(&f.hub, &f.late_out, 1).await.unwrap(),
        Some(vec![f.hub.clone(), f.late_out.clone()])
    );
    let prepared = serde_json::from_value::<PathQuery>(args)
        .unwrap()
        .prepare()
        .unwrap();
    let value = Box::pin(prepared.read(async {
        let mut handle = PgReader::new(config);
        handle
            .connect()
            .await
            .map_err(|error| chaosbox::PipelineError::Consumer(error.to_string()))?;
        Ok(handle)
    }))
    .await
    .unwrap();
    assert_eq!(value["path"], Value::Null);
    client.stop().await;
    eprintln!(
        "{}",
        json!({"case":"ordinary-path-clients","old":f.build.id,"current":new.id,
        "nodes":f.build.nodes.len(),"relationships":f.build.edges.len(),"outgoing_hub_rows":227,"incoming_hub_rows":427,
        "cases_per_transport":cases + 1,"transports":["fresh-cli","fresh-mcp","persistent-mcp"],
        "already_pinned_and_fresh_historical_reads":true})
    );
}
