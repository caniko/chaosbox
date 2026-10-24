//! Ordinary clients must expose bounded, exact-publication continuation.

use std::{process::Stdio, time::Duration};
use chaosbox_core::{GraphBuild, SnapshotFile};
use chaosbox_store::{NeighborhoodDirection, Store};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

#[path = "support/neighborhood.rs"]
mod fixture;
use fixture::{DEGREE, Fixture, REPO, anchored, fixture, projection};
#[path = "../../chaosbox-pg/tests/support/server.rs"]
mod server;

thread_local! {
    /// Disposable database of the live test on this thread; spawned clients use it.
    static DATABASE_URL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn database_url() -> String {
    DATABASE_URL.with_borrow(|url| url.clone().expect("live test starts its server first"))
}

fn binary() -> std::ffi::OsString {
    std::env::var_os("CHAOSBOX_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_chaosbox").into())
}

#[tokio::test]
async fn cli_validates_paging_before_credentials() {
    for args in [
        vec!["--limit", "0"],
        vec!["--limit", "201"],
        vec!["--cursor", "broken"],
    ] {
        let output = tokio::process::Command::new(binary())
            .args(["query", "neighbors", "ent:hub", "--repo", "demo"])
            .args(args)
            .env_remove("CHAOSBOX_DATABASE_URL")
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("invalid neighborhood"),
            "{text}; {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("CHAOSBOX_DATABASE_URL"));
    }
}

#[tokio::test]
async fn cli_validates_exact_publications_before_credentials() {
    for name in ["lookup", "explain", "neighbors"] {
        for build in [
            String::new(),
            " ".into(),
            "build:\ncontrol".into(),
            "é".repeat(129),
        ] {
            let output = tokio::process::Command::new(binary())
                .args([
                    "query",
                    name,
                    "ent:fixture",
                    "--repo",
                    "demo",
                    "--build-id",
                    &build,
                ])
                .env_remove("CHAOSBOX_DATABASE_URL")
                .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
                .kill_on_drop(true)
                .output()
                .await
                .unwrap();
            assert!(!output.status.success());
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(
                text.contains("invalid build_id"),
                "{text}; {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!text.contains("CHAOSBOX_DATABASE_URL"));
        }
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
        Self::start_binary(binary()).await
    }

    async fn start_binary(binary: std::ffi::OsString) -> Self {
        let mut child = tokio::process::Command::new(binary)
            .arg("mcp")
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .env("CHAOSBOX_DATABASE_URL", database_url())
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
            .rpc(
                "initialize",
                json!({"protocolVersion":"2025-06-18",
            "capabilities":{},"clientInfo":{"name":"neighborhood-regression","version":"1"}}),
            )
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

    async fn page(&mut self, args: &Value) -> Value {
        self.call("neighbors", args).await
    }

    async fn call(&mut self, name: &str, args: &Value) -> Value {
        let response = self
            .rpc("tools/call", json!({"name":name,"arguments":args}))
            .await;
        assert!(response.get("error").is_none(), "{response}");
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
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

async fn fresh_page(args: &Value) -> Value {
    let mut client = Mcp::start().await;
    let page = client.page(args).await;
    client.stop().await;
    page
}

async fn cli_output(args: &Value) -> std::process::Output {
    cli_query_output("neighbors", args).await
}

async fn cli_query_output(name: &str, args: &Value) -> std::process::Output {
    let mut command = tokio::process::Command::new(binary());
    command.args([
        "query",
        name,
        args["id"].as_str().unwrap(),
        "--repo",
        args["repo"].as_str().unwrap(),
    ]);
    for key in ["limit", "cursor", "direction", "rel", "build_id"] {
        if let Some(value) = args.get(key) {
            command.arg(format!("--{}", key.replace('_', "-"))).arg(
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned),
            );
        }
    }
    if let Some(kinds) = args.get("rel_types") {
        command.arg("--rel-types").args(
            kinds
                .as_array()
                .unwrap()
                .iter()
                .map(|kind| kind.as_str().unwrap()),
        );
    }
    tokio::time::timeout(
        Duration::from_secs(30),
        command
            .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
            .env("CHAOSBOX_DATABASE_URL", database_url())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn cli_page(args: &Value) -> Value {
    let output = cli_output(args).await;
    assert!(
        output.status.success(),
        "{}; {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn identical_pages(client: &mut Mcp, args: &Value) -> Value {
    let page = cli_page(args).await;
    assert_eq!(fresh_page(args).await, page, "fresh MCP differs from CLI");
    assert_eq!(
        client.page(args).await,
        page,
        "persistent MCP differs from CLI"
    );
    page
}

async fn identical_followup(client: &mut Mcp, name: &str, args: &Value) -> Value {
    let value = client.call(name, args).await;
    let mut fresh = Mcp::start().await;
    assert_eq!(fresh.call(name, args).await, value, "fresh {name}");
    fresh.stop().await;
    if name != "evidence" {
        let output = cli_query_output(name, args).await;
        assert!(
            output.status.success(),
            "{}; {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            value,
            "CLI {name}"
        );
    }
    value
}

async fn historical_fixture(
    store: &mut chaosbox_pg::store::PgStore,
) -> (GraphBuild, GraphBuild, Value) {
    use chaosbox_core::{
        Entity, EntityKind, Evidence, EvidenceClass, Relation, RelationScope, RelationType,
        SourceSpan,
    };
    const SOURCE: &str =
        "fn hub() { leaf(); tip(); }\nfn leaf() { tip(); }\nfn tip() { leaf(); }\n";
    let hash = chaosbox_core::sha256_hex(&[SOURCE]);
    store
        .ensure_snapshot_files(
            "s-historical",
            REPO,
            &[SnapshotFile {
                snapshot: "s-historical".into(),
                path: "f.rs".into(),
                sha256: hash.clone(),
                bytes: SOURCE.len() as u64,
            }],
        )
        .await
        .unwrap();
    let mut span = SourceSpan::point("f.rs", 1, 1, 0);
    span.end_line = 4;
    span.byte_end = u32::try_from(SOURCE.len()).unwrap();
    store
        .put_evidence(Evidence {
            id: "ev:historical-source".into(),
            class: EvidenceClass::Extracted,
            supports: true,
            text: SOURCE.into(),
            span: Some(span.clone()),
            snapshot: "s-historical".into(),
            source_file_version: "f.rs".into(),
            producer: Some("historical-fixture/1".into()),
        })
        .await
        .unwrap();
    let nodes = ["hub", "leaf", "tip"].map(|name| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            "s-historical",
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    });
    let mut old = GraphBuild::new(REPO, vec!["s-historical".into()], 1);
    for node in &nodes {
        old.add_node(node.clone()).unwrap();
    }
    for (from, to) in [(0, 1), (0, 2), (1, 2), (2, 1)] {
        let mut edge = Relation::new(
            RelationType::Calls,
            &nodes[from].id,
            &nodes[to].id,
            RelationScope::File,
            &old.id,
        );
        edge.evidence_ids.push("ev:historical-source".into());
        old.add_edge(edge).unwrap();
    }
    let mut new = GraphBuild::new(REPO, old.snapshot_ids.clone(), 2);
    new.predecessor = Some(old.id.clone());
    new.add_node(nodes[0].clone()).unwrap();
    store.publish(old.clone(), None).await.unwrap();
    // Expected source rows are defined from fixture bytes, never another reader.
    let evidence = json!([{"evidence_id":"ev:historical-source", "class":"extracted", "supports":true,
        "text":SOURCE, "producer":"historical-fixture/1",
        "citation":{"snapshot":"s-historical", "file":"f.rs", "sha256":hash, "span":span}}]);
    (old, new, evidence)
}

async fn reject_exact_builds(client: &mut Mcp, args: &Value, foreign: &str) {
    for build in ["build:missing", foreign] {
        for name in ["lookup", "explain", "evidence", "neighbors"] {
            let mut args = args.clone();
            args.as_object_mut()
                .unwrap()
                .remove(if name == "evidence" { "id" } else { "rel" });
            args["build_id"] = json!(build);
            let response = client
                .rpc("tools/call", json!({"name":name,"arguments":args}))
                .await;
            assert_eq!(response["error"]["code"], -32603, "{name}: {response}");
            assert!(response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("published build unavailable"));
            let mut fresh = Mcp::start().await;
            let fresh_error = fresh
                .rpc("tools/call", json!({"name":name,"arguments":args}))
                .await;
            assert_eq!(fresh_error["error"], response["error"]);
            fresh.stop().await;
            if name != "evidence" {
                let output = cli_query_output(name, &args).await;
                assert!(!output.status.success());
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("published build unavailable")
                );
            }
        }
    }
}

async fn expand_historical(client: &mut Mcp, old: &GraphBuild, endpoint: &str) {
    // Expand a returned historical endpoint, then expand its returned neighbor.
    let mut anchor = endpoint.to_owned();
    for _ in 0..2 {
        let request = json!({"repo":REPO,"id":anchor,"build_id":old.id});
        let page = identical_pages(client, &request).await;
        assert_eq!(page["build_id"], old.id);
        assert_eq!(page["complete"], true);
        for (key, direction) in [
            ("outgoing", NeighborhoodDirection::Outgoing),
            ("incoming", NeighborhoodDirection::Incoming),
        ] {
            let expected = old
                .edges
                .values()
                .filter(|edge| anchored(edge, &anchor, direction))
                .map(projection)
                .collect::<Vec<_>>();
            assert_eq!(page[key], json!(expected));
        }
        anchor = page["outgoing"][0]["to_entity"]["entity_id"]
            .as_str()
            .unwrap()
            .into();
    }
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_clients_hydrate_and_expand_exact_historical_publications() {
    use chaosbox_pg::{store::PgStore, PgConfig};
    let server = server::Server::start();
    DATABASE_URL.set(Some(server.url()));
    let mut store = PgStore::new(PgConfig::new(server.url()).unwrap());
    store.migrate().await.unwrap();
    let (old, new, evidence) = historical_fixture(&mut store).await;
    let hub = old.nodes.values().find(|node| node.name == "hub").unwrap();
    let root = json!({"repo":REPO,"id":hub.id,"limit":1});
    // Pre-0.2 version-1 cursor producers only read TypeDB; a fresh current
    // client stands in, and an unrequested build scope must still be absent.
    let legacy_page = fresh_page(&root).await;
    let cursor = legacy_page["next_cursor"].as_str().unwrap();
    assert!(serde_json::from_str::<Value>(cursor).unwrap()["scope"]
        .get("build_id")
        .is_none());
    let mut client = Mcp::start().await;
    assert_eq!(client.page(&root).await, legacy_page);
    store
        .publish(new.clone(), Some(old.id.clone()))
        .await
        .unwrap();
    let edge = &legacy_page["outgoing"][0];
    let endpoint = &old.nodes[edge["to_entity"]["entity_id"].as_str().unwrap()];
    let args = json!({"repo":REPO,"id":endpoint.id,"rel":edge["rel_id"]});
    for name in ["lookup", "explain"] {
        let mut request = args.clone();
        request.as_object_mut().unwrap().remove("rel");
        assert_eq!(
            identical_followup(&mut client, name, &request).await,
            Value::Null
        );
        request["build_id"] = json!(old.id);
        let expected = if name == "lookup" {
            json!({"entity_id":endpoint.id,"kind":"symbol","repo":REPO,"snapshot":"s-historical",
                "file":"f.rs","name":endpoint.name,"qualified_name":endpoint.qualified_name,"span":endpoint.span})
        } else {
            json!({"id":endpoint.id,"kind":"symbol","file":"f.rs","qualified_name":endpoint.qualified_name,
                "snapshot":"s-historical","span":endpoint.span,"compiler":null,"outgoing":1,"incoming":2})
        };
        assert_eq!(
            client.call(name, &request).await,
            expected,
            "historical {name}"
        );
        assert_eq!(
            identical_followup(&mut client, name, &request).await,
            expected
        );
    }
    let evidence_args = json!({"repo":REPO,"rel":edge["rel_id"]});
    assert_eq!(
        identical_followup(&mut client, "evidence", &evidence_args).await["evidence"],
        json!([])
    );
    let mut exact_evidence = evidence_args;
    exact_evidence["build_id"] = json!(old.id);
    assert_eq!(
        identical_followup(&mut client, "evidence", &exact_evidence).await,
        json!({"rel":edge["rel_id"],"evidence":evidence})
    );
    expand_historical(&mut client, &old, &endpoint.id).await;
    let mut continuation = root.clone();
    continuation["cursor"] = json!(cursor);
    let omitted = identical_pages(&mut client, &continuation).await;
    continuation["build_id"] = json!(old.id);
    assert_eq!(identical_pages(&mut client, &continuation).await, omitted);
    let mut exact_root = root.clone();
    exact_root["build_id"] = json!(old.id);
    assert_eq!(identical_pages(&mut client, &exact_root).await, legacy_page);
    continuation["build_id"] = json!(new.id);
    let conflict = client
        .rpc(
            "tools/call",
            json!({"name":"neighbors","arguments":continuation}),
        )
        .await;
    assert_eq!(conflict["error"]["code"], -32602, "{conflict}");
    let foreign = GraphBuild::new("foreign", vec![], 1);
    store.publish(foreign.clone(), None).await.unwrap();
    // Use a currently present endpoint so a mistaken fallback cannot hide behind null.
    let mut rejected = args.clone();
    rejected["id"] = json!(hub.id);
    reject_exact_builds(&mut client, &rejected, &foreign.id).await;
    client.stop().await;
    eprintln!(
        "{}",
        json!({"case":"historical-followups","old":old.id,"current":new.id,
        "historical_endpoint":endpoint.id,"evidence":evidence,"recursive_expansions":2,"legacy_cursor":cursor,
        "transports":["fresh-cli","fresh-mcp","persistent-mcp"]})
    );
}

fn expected(f: &Fixture, args: &Value, direction: NeighborhoodDirection) -> Vec<Value> {
    let mode = args
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("both");
    if (mode == "incoming" && direction == NeighborhoodDirection::Outgoing)
        || (mode == "outgoing" && direction == NeighborhoodDirection::Incoming)
    {
        return Vec::new();
    }
    let kinds = args
        .get("rel_types")
        .map(|kinds| {
            kinds
                .as_array()
                .unwrap()
                .iter()
                .map(|kind| kind.as_str().unwrap().to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            args.get("rel")
                .map(|kind| vec![kind.as_str().unwrap().to_ascii_lowercase()])
        });
    f.builds[1]
        .edges
        .values()
        .filter(|edge| {
            anchored(edge, args["id"].as_str().unwrap(), direction)
                && kinds.as_ref().is_none_or(|kinds| {
                    kinds.contains(&chaosbox_core::relation_type_name(&edge.rel_type))
                })
        })
        .map(projection)
        .collect()
}

async fn enumerate(
    client: &mut Mcp,
    f: &Fixture,
    requests: Vec<Value>,
    first_pages: Vec<Value>,
) -> (usize, usize, usize) {
    let mut total_pages = 0;
    let mut total_rows = 0;
    let mut total_bytes = 0;
    for (mut request, mut page) in requests.into_iter().zip(first_pages) {
        let expected = [
            expected(f, &request, NeighborhoodDirection::Outgoing),
            expected(f, &request, NeighborhoodDirection::Incoming),
        ];
        let mut rows = [Vec::new(), Vec::new()];
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(pages < 100, "cursor did not finish");
            assert_eq!(page["build_id"], f.builds[1].id);
            assert_eq!(page["generation"], 2);
            assert_eq!(page["repo"], REPO);
            assert_eq!(page["id"], request["id"]);
            let size = request
                .get("limit")
                .map_or(20, |limit| limit.as_u64().unwrap());
            for (index, key) in ["outgoing", "incoming"].iter().enumerate() {
                let delivered = page[key].as_array().unwrap();
                assert!(delivered.len() <= usize::try_from(size).unwrap());
                assert!(delivered
                    .windows(2)
                    .all(|pair| pair[0]["rel_id"].as_str() < pair[1]["rel_id"].as_str()));
                rows[index].extend(delivered.iter().cloned());
                assert_eq!(
                    page[format!("{key}_complete")],
                    rows[index].len() == expected[index].len()
                );
                total_rows += delivered.len();
            }
            total_bytes += serde_json::to_vec(&page).unwrap().len();
            assert_eq!(page["complete"], page["next_cursor"].is_null());
            if page["complete"] == true {
                break;
            }
            request["cursor"] = page["next_cursor"].clone();
            // Legacy scalar and normalized list filters share the same continuation.
            if request.get("rel").is_some() {
                request.as_object_mut().unwrap().remove("rel");
                request["rel_types"] = json!(["references", "REFERENCES"]);
            }
            page = identical_pages(client, &request).await;
        }
        assert_eq!(rows, expected);
        for edge in rows.iter().flatten() {
            for endpoint in ["from_entity", "to_entity"] {
                assert!(f.builds[1]
                    .nodes
                    .contains_key(edge[endpoint]["entity_id"].as_str().unwrap()));
            }
        }
        total_pages += pages;
    }
    (total_pages, total_rows, total_bytes)
}

async fn check_cursor_rejections(client: &mut Mcp, f: &Fixture, original_cursor: &str) {
    // A real cursor is rejected for scope changes before any backend operation.
    for (key, value) in [
        ("repo", json!("pages-other")),
        ("id", json!(f.isolated)),
        ("limit", json!(127)),
        ("direction", json!("incoming")),
        ("rel_types", json!(["calls"])),
    ] {
        let mut wrong = json!({"repo":REPO,"id":f.hub,"limit":128,"cursor":original_cursor});
        wrong[key] = value;
        let response = client
            .rpc("tools/call", json!({"name":"neighbors","arguments":wrong}))
            .await;
        assert_eq!(response["error"]["code"], -32602, "{response}");
        let output = cli_output(&wrong).await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("invalid neighborhood cursor"));
    }
    let mut cursor: Value = serde_json::from_str(original_cursor).unwrap();
    cursor["build_id"] = json!("build:missing");
    let unavailable = json!({"repo":REPO,"id":f.hub,"limit":128,"cursor":cursor.to_string()});
    let response = client
        .rpc(
            "tools/call",
            json!({"name":"neighbors","arguments":unavailable}),
        )
        .await;
    assert_eq!(response["error"]["code"], -32603, "{response}");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("published build unavailable"));
    let output = cli_output(&unavailable).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("published build unavailable"));
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_clients_enumerate_exact_historical_pages() {
    use chaosbox_pg::{store::PgStore, PgConfig};

    let server = server::Server::start();
    DATABASE_URL.set(Some(server.url()));
    let mut store = PgStore::new(PgConfig::new(server.url()).unwrap());
    store.migrate().await.unwrap();
    for (snapshot, repo) in [("s-pages", REPO), ("s-other", "pages-other")] {
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
    for build in &f.builds {
        store
            .publish(build.clone(), build.predecessor.clone())
            .await
            .unwrap();
    }
    let mut client = Mcp::start().await;
    let tools = client.rpc("tools/list", json!({})).await;
    let schema = &tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "neighbors")
        .unwrap()["inputSchema"];
    assert_eq!(schema["properties"]["limit"]["default"], 20);
    let default = identical_pages(&mut client, &json!({"repo":REPO,"id":f.hub})).await;
    assert_eq!(default["outgoing"].as_array().unwrap().len(), 20);
    assert_eq!(default["incoming"].as_array().unwrap().len(), 20);
    assert_eq!(default["complete"], false);
    let requests = vec![
        json!({"repo":REPO,"id":f.hub,"limit":128}),
        json!({"repo":REPO,"id":f.hub,"limit":1,"rel":"REFERENCES"}),
        json!({"repo":REPO,"id":f.hub,"limit":200,"direction":"incoming"}),
        json!({"repo":REPO,"id":f.hub,"limit":200,"direction":"outgoing"}),
        json!({"repo":REPO,"id":f.hub,"limit":1,"rel_types":["defines"]}),
        json!({"repo":REPO,"id":f.hub,"rel_types":[]}),
        json!({"repo":REPO,"id":f.isolated}),
        json!({"repo":REPO,"id":f.foreign}),
        json!({"repo":REPO,"id":"ent:missing"}),
    ];
    let mut first_pages = Vec::new();
    for request in &requests {
        first_pages.push(identical_pages(&mut client, request).await);
    }
    let mut new = GraphBuild::new(REPO, vec!["s-pages".into()], 3);
    new.predecessor = Some(f.builds[1].id.clone());
    new.add_node(f.builds[1].nodes[&f.hub].clone()).unwrap();
    store
        .publish(new.clone(), new.predecessor.clone())
        .await
        .unwrap();
    let current = identical_pages(&mut client, &json!({"repo":REPO,"id":f.hub})).await;
    assert_eq!(current["build_id"], new.id);
    assert_eq!(current["complete"], true);
    assert_eq!(current["outgoing"], json!([]));
    assert_eq!(current["incoming"], json!([]));
    let original_cursor = first_pages[0]["next_cursor"].as_str().unwrap().to_owned();
    let (total_pages, total_rows, total_bytes) =
        enumerate(&mut client, &f, requests, first_pages).await;
    check_cursor_rejections(&mut client, &f, &original_cursor).await;
    client.stop().await;
    assert_eq!(
        expected(&f, &json!({"id":f.hub}), NeighborhoodDirection::Outgoing).len(),
        DEGREE + 5
    );
    eprintln!(
        "{}",
        json!({"case":"ordinary-client-enumeration","old":f.builds[1].id,"current":new.id,
        "fixture_nodes":f.builds[1].nodes.len(),"fixture_edges":f.builds[1].edges.len(),
        "pages_per_transport":total_pages,"rows_per_transport":total_rows,"page_json_bytes_per_transport":total_bytes,
        "transports":["fresh-cli","fresh-mcp","persistent-mcp"]})
    );
}
