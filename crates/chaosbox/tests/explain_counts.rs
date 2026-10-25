//! Count-only explanations must not materialize relationship collections.

use chaosbox::{GraphReader, all_relation_types};
use chaosbox_core::{Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SourceSpan};
use chaosbox_store::{BuildRow, EntityRow, EvidenceRow, GraphQueries, MemoryReader, RelRow, StoreError};
use serde_json::{Value, json};

const DEGREE: usize = 1024;
const REPO: &str = "explain-counts";

fn fixture() -> (GraphBuild, GraphBuild, Entity, Entity) {
    let entity = |name: &str| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            "s-counts",
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = entity("hub");
    let isolated = entity("isolated");
    let first = entity("leaf-0");
    let mut old = GraphBuild::new(REPO, vec!["s-counts".into()], 1);
    for node in [&hub, &isolated, &first] {
        old.add_node(node.clone()).unwrap();
    }
    edge(&mut old, RelationType::Calls, &hub.id, &first.id);
    let mut build = GraphBuild::new(REPO, vec!["s-counts".into()], 2);
    build.predecessor = Some(old.id.clone());
    build.add_node(hub.clone()).unwrap();
    build.add_node(isolated.clone()).unwrap();
    for index in 0..DEGREE {
        let leaf = entity(&format!("leaf-{index}"));
        build.add_node(leaf.clone()).unwrap();
        edge(&mut build, RelationType::Calls, &hub.id, &leaf.id);
    }
    edge(&mut build, RelationType::References, &hub.id, &first.id);
    edge(&mut build, RelationType::References, &first.id, &hub.id);
    edge(&mut build, RelationType::Calls, &hub.id, &hub.id);
    (old, build, hub, isolated)
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

fn expected(entity: &Entity, outgoing: usize, incoming: usize) -> Value {
    json!({
        "id":entity.id, "kind":"symbol", "file":entity.file,
        "qualified_name":entity.qualified_name, "snapshot":entity.snapshot,
        "span":entity.span, "compiler":entity.compiler,
        "outgoing":outgoing, "incoming":incoming,
    })
}

struct CountsOnly {
    reader: MemoryReader,
    fail_counts: bool,
}

#[async_trait::async_trait]
impl GraphQueries for CountsOnly {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError> {
        self.reader.active_build(repo).await
    }
    async fn published_build(&self, _: &str, _: &str) -> Result<Option<BuildRow>, StoreError> {
        panic!("explanation must not load historical headers")
    }
    async fn search_entities(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        panic!("explanation must not search entities")
    }
    async fn entity_by_id(&self, build: &str, id: &str) -> Result<Option<EntityRow>, StoreError> {
        self.reader.entity_by_id(build, id).await
    }
    async fn neighbor_counts(
        &self,
        build: &str,
        id: &str,
        kinds: Vec<String>,
    ) -> Result<(u64, u64), StoreError> {
        if self.fail_counts {
            return Err(StoreError::Query("count failed".into()));
        }
        self.reader.neighbor_counts(build, id, kinds).await
    }
    async fn neighbors_out(
        &self,
        _: &str,
        _: &str,
        _: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        panic!("explanation must not materialize outgoing relationships")
    }
    async fn neighbors_in(
        &self,
        _: &str,
        _: &str,
        _: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        panic!("explanation must not materialize incoming relationships")
    }
    async fn build_entities(&self, _: &str, _: i64) -> Result<Vec<EntityRow>, StoreError> {
        panic!("explanation must not export entities")
    }
    async fn build_relationships(&self, _: &str, _: i64) -> Result<Vec<RelRow>, StoreError> {
        panic!("explanation must not export relationships")
    }
    async fn evidence_for(&self, _: &str, _: &str) -> Result<Vec<EvidenceRow>, StoreError> {
        panic!("explanation must not load evidence")
    }
    async fn evidence_for_limited(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        panic!("explanation must not load evidence")
    }
}

#[tokio::test]
async fn explain_preserves_exact_counts_without_relationship_collections() {
    let (old, build, hub, isolated) = fixture();
    let mut memory = MemoryReader::new();
    let (old_id, build_id) = (old.id.clone(), build.id.clone());
    memory.insert_build(old);
    memory.insert_build(build);
    // Publishing is `set_active`; the old generation stays readable by pin.
    memory.set_active(REPO, &old_id);
    memory.set_active(REPO, &build_id);
    check_counts(&memory, &hub, &isolated).await;
    let reader = GraphReader::pinned(
        CountsOnly {
            reader: memory,
            fail_counts: false,
        },
        REPO,
    )
    .await
    .unwrap();
    assert_eq!(
        reader.explain(&hub.id).await.unwrap(),
        expected(&hub, DEGREE + 2, 2)
    );
    assert_eq!(
        reader.explain(&isolated.id).await.unwrap(),
        expected(&isolated, 0, 0)
    );
    assert_eq!(reader.explain("ent:missing").await.unwrap(), Value::Null);
}

#[tokio::test]
async fn explain_count_failure_propagates_without_collection_fallback() {
    let seed = chaosbox_store::conformance_seed();
    let reader = GraphReader::pinned(
        CountsOnly {
            reader: seed.reader,
            fail_counts: true,
        },
        "conf",
    )
    .await
    .unwrap();
    assert!(reader
        .explain(&seed.a2)
        .await
        .unwrap_err()
        .to_string()
        .contains("count failed"));
    assert_eq!(reader.explain(&seed.a1).await.unwrap(), Value::Null);
}

async fn check_counts(reader: &impl GraphQueries, hub: &Entity, isolated: &Entity) {
    let (old, build, _, _) = fixture();
    let active_only = build
        .nodes
        .values()
        .find(|node| node.name == "leaf-1")
        .unwrap();
    let all = all_relation_types();
    assert_eq!(
        reader
            .neighbor_counts(&build.id, &hub.id, all.clone())
            .await
            .unwrap(),
        (DEGREE as u64 + 2, 2)
    );
    assert_eq!(
        reader
            .neighbor_counts(&old.id, &hub.id, all.clone())
            .await
            .unwrap(),
        (1, 0)
    );
    for (selected_build, id) in [
        (build.id.as_str(), isolated.id.as_str()),
        (build.id.as_str(), "ent:foreign"),
        (old.id.as_str(), active_only.id.as_str()),
        ("build:missing", hub.id.as_str()),
    ] {
        assert_eq!(
            reader
                .neighbor_counts(selected_build, id, all.clone())
                .await
                .unwrap(),
            (0, 0)
        );
    }
    for (kinds, expected) in [
        (vec![], (0, 0)),
        (vec!["calls".into(), "calls".into()], (DEGREE as u64 + 1, 1)),
        (vec!["references".into()], (1, 1)),
        (vec!["\"; delete $x isa code-entity;".into()], (0, 0)),
    ] {
        assert_eq!(
            reader
                .neighbor_counts(&build.id, &hub.id, kinds)
                .await
                .unwrap(),
            expected
        );
    }
}

#[tokio::test]
#[ignore = "requires an explicit disposable TypeDB database named s02a_* and CHAOSBOX_REQUIRE_TYPEDB=1"]
async fn disposable_counts_and_native_cli_mcp_preserve_explanations() {
    use chaosbox_core::SnapshotFile;
    use chaosbox_store::Store;
    use chaosbox_typedb::{
        reader::TypeDbReader,
        store::{TypeDbConfig, TypeDbStore},
    };

    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    assert!(
        config.database.starts_with("s02a_"),
        "use a disposable s02a_* database"
    );
    let mut store = TypeDbStore::new(config.clone());
    store.migrate().await.unwrap();
    store
        .ensure_snapshot_files(
            "s-counts",
            REPO,
            &[SnapshotFile {
                snapshot: "s-counts".into(),
                path: "f.rs".into(),
                sha256: "fixture".into(),
                bytes: 1,
            }],
        )
        .await
        .unwrap();
    let (old, build, hub, isolated) = fixture();
    store.publish(old.clone(), None).await.unwrap();
    store
        .publish(build.clone(), Some(old.id.clone()))
        .await
        .unwrap();
    let mut backend = TypeDbReader::new(config);
    backend.connect().await.unwrap();
    check_counts(&backend, &hub, &isolated).await;
    let reader = GraphReader::pinned(backend, REPO).await.unwrap();
    assert_eq!(
        reader.explain(&hub.id).await.unwrap(),
        expected(&hub, DEGREE + 2, 2)
    );

    // All calls below share one MCP process; CLI runs use the same exact executable.
    let ids = [&hub.id, &isolated.id, "ent:missing"];
    let mut requests = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"count-regression","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ];
    for (index, id) in ids.iter().cycle().take(9).enumerate() {
        requests.push(json!({"jsonrpc":"2.0","id":index+2,"method":"tools/call","params":{"name":"explain","arguments":{"repo":REPO,"id":id}}}));
    }
    let responses = native(&["mcp"], &requests).await;
    assert_eq!(responses.len(), 10);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
    let expected_values = [
        expected(&hub, DEGREE + 2, 2),
        expected(&isolated, 0, 0),
        Value::Null,
    ];
    let mut total_bytes = 0;
    for (index, response) in responses[1..].iter().enumerate() {
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["id"], index + 2);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        total_bytes += text.len();
        assert_eq!(payload, expected_values[index % 3]);
    }
    for (id, value) in ids.iter().zip(&expected_values) {
        assert_eq!(
            native(&["query", "explain", id, "--repo", REPO], &[])
                .await
                .as_slice(),
            std::slice::from_ref(value)
        );
    }
    eprintln!(
        "{}",
        json!({"fixture_database":std::env::var("CHAOSBOX_TYPEDB_DATABASE").unwrap(),
        "build":build.id,"nodes":build.nodes.len(),"edges":build.edges.len(),
        "mcp_calls":9,"mcp_payload_bytes":total_bytes,"cli_cases":3})
    );
}

async fn native(args: &[&str], requests: &[Value]) -> Vec<Value> {
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(args)
        .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = args
        .first()
        .filter(|arg| **arg == "mcp")
        .map(|_| BufReader::new(child.stdout.take().unwrap()).lines());
    let mut replies = Vec::new();
    for request in requests {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        if let Some(stdout) = stdout.as_mut().filter(|_| request.get("id").is_some()) {
            let line = tokio::time::timeout(Duration::from_secs(30), stdout.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], request["id"]);
            replies.push(response);
        }
    }
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("native explanation exceeded 60 seconds")
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if stdout.is_some() {
        return replies;
    }
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
