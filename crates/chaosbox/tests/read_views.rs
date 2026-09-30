//! Authorization and version isolation for the trusted connector's reader.

use chaosbox::read_view::{Budgets, ReadView, RunIdentity, ScopedReader};
use chaosbox_store::{conformance_seed, GraphQueries, EvidenceRow, SourceCitation};
use serde_json::json;

fn view(build: &str, snapshots: &[&str]) -> ReadView {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    serde_json::from_value(json!({
        "version": 1, "id": "view-a", "policy_revision": "policy-1",
        "identity": {"company": "c", "project": "p", "agent": "a", "task": "t",
            "run": "run-a", "host": "atlas", "server": "chaosbox"},
        "issued_at": now, "expires_at": now + 3600,
        "repo": "conf", "build_id": build, "snapshots": snapshots,
        "source_paths": ["."], "excluded_paths": [],
        "operations": ["status", "search", "lookup", "neighbors", "path", "explain", "evidence", "export"],
        "budgets": Budgets::default()
    })).unwrap()
}

async fn cited_seed() -> chaosbox_store::ConformanceSeed {
    let mut seed = conformance_seed();
    for (build, snapshot) in [(&seed.builds.0, "s1"), (&seed.builds.1, "s2")] {
        for rel in seed.reader.build_relationships(build, 10).await.unwrap() {
            seed.reader.attach_evidence(
                &rel.rel_id,
                &[EvidenceRow {
                    evidence_id: format!("ev:{snapshot}"),
                    class: "extracted".into(),
                    supports: true,
                    text: "source excerpt".into(),
                    producer: Some("test-parser-v1".into()),
                    citation: Some(SourceCitation {
                        snapshot: snapshot.into(),
                        file: "f.rs".into(),
                        sha256: "a".repeat(64),
                        span: None,
                    }),
                }],
            );
        }
    }
    seed
}

#[tokio::test]
async fn pinned_historical_view_never_follows_active_pointer() {
    let seed = cited_seed().await;
    let view = view(&seed.builds.0, &["s1"]);
    let identity = view.identity.clone();
    let reader = ScopedReader::admit(seed.reader, view, &identity)
        .await
        .unwrap();
    let hits = reader
        .call(&identity, "search", json!({"repo":"conf", "query":"alpha"}))
        .await
        .unwrap();
    assert_eq!(hits["data"]["items"][0]["entity_id"], seed.a1);
    assert_eq!(hits["read_view"]["build_id"], seed.builds.0);
    assert_eq!(hits["applicability"], "unknown");
    assert!(hits["workspace_observation"].is_null());
    let invisible = reader
        .call(&identity, "lookup", json!({"repo":"conf", "id":seed.a2}))
        .await
        .unwrap();
    assert!(invisible["data"].is_null());
}

#[tokio::test]
async fn identities_operations_and_arguments_cannot_expand_admission() {
    let seed = cited_seed().await;
    let mut view = view(&seed.builds.1, &["s2"]);
    view.operations
        .retain(|op| serde_json::to_value(op).unwrap() == "status");
    let identity = view.identity.clone();
    let reader = ScopedReader::admit(seed.reader, view, &identity)
        .await
        .unwrap();
    let mut other: RunIdentity = identity.clone();
    other.run = "run-b".into();
    assert!(
        reader
            .call(&other, "status", json!({"repo":"conf"}))
            .await
            .is_err()
    );
    assert!(
        reader
            .call(&identity, "export", json!({"repo":"conf"}))
            .await
            .is_err()
    );
    assert!(
        reader
            .call(&identity, "status", json!({"repo":"other"}))
            .await
            .is_err()
    );
    assert!(
        reader
            .call(
                &identity,
                "status",
                json!({"repo":"conf", "build_id":"secret"})
            )
            .await
            .is_err()
    );
    assert!(
        reader
            .call(&identity, "run", json!({"repo":"conf"}))
            .await
            .is_err()
    );
    assert!(
        reader
            .call(&identity, "status", json!({"repo":"conf"}))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn complete_evidence_scope_is_required() {
    let seed = conformance_seed();
    let view = view(&seed.builds.0, &["s1"]);
    let identity = view.identity.clone();
    // The legacy fixture evidence has no citation. Do not release its text.
    let error = ScopedReader::admit(seed.reader, view, &identity)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), "evidence_scope_unavailable");
}

#[tokio::test]
async fn revoked_view_cannot_be_reused() {
    let seed = cited_seed().await;
    let view = view(&seed.builds.1, &["s2"]);
    let identity = view.identity.clone();
    let reader = ScopedReader::admit(seed.reader, view, &identity)
        .await
        .unwrap();
    reader.revoker().revoke();
    let error = reader
        .call(&identity, "status", json!({"repo":"conf"}))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "revoked");
}

#[tokio::test]
async fn snapshots_must_match_recorded_version() {
    let seed = conformance_seed();
    let view = view(&seed.builds.1, &["s1"]);
    let identity = view.identity.clone();
    assert!(
        ScopedReader::admit(seed.reader, view, &identity)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pagination_is_deterministic_and_bound_to_the_complete_view() {
    let seed = cited_seed().await;
    let grant = view(&seed.builds.0, &["s1"]);
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant.clone(), &identity)
        .await
        .unwrap();
    let first = reader
        .call(
            &identity,
            "export",
            json!({"repo":"conf", "kind":"nodes", "limit":1}),
        )
        .await
        .unwrap();
    let cursor = first["data"]["next_cursor"].clone();
    assert!(cursor.is_string());
    let second = reader
        .call(
            &identity,
            "export",
            json!({"repo":"conf", "kind":"nodes", "limit":1, "cursor":cursor}),
        )
        .await
        .unwrap();
    assert_ne!(
        first["data"]["items"][0]["entity_id"],
        second["data"]["items"][0]["entity_id"]
    );
    assert!(second["data"]["next_cursor"].is_null());
    assert!(
        reader
            .call(
                &identity,
                "export",
                json!({"repo":"conf", "kind":"edges", "limit":1, "cursor":cursor})
            )
            .await
            .is_err()
    );

    let seed = cited_seed().await;
    let mut changed = grant;
    changed.policy_revision = "narrower-policy".into();
    let reader = ScopedReader::admit(seed.reader, changed, &identity)
        .await
        .unwrap();
    assert!(
        reader
            .call(
                &identity,
                "export",
                json!({"repo":"conf", "kind":"nodes", "limit":1, "cursor":cursor})
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn source_exclusions_cover_every_query_including_self_paths() {
    let seed = cited_seed().await;
    let hidden = seed
        .reader
        .build_entities(&seed.builds.1, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.file == "g.rs")
        .unwrap();
    let mut view = view(&seed.builds.1, &["s2"]);
    view.excluded_paths.push("g.rs".into());
    let identity = view.identity.clone();
    let reader = ScopedReader::admit(seed.reader, view, &identity)
        .await
        .unwrap();
    for name in ["lookup", "explain"] {
        assert!(
            reader
                .call(
                    &identity,
                    name,
                    json!({"repo":"conf", "id":hidden.entity_id})
                )
                .await
                .unwrap()["data"]
                .is_null()
        );
    }
    for args in [
        json!({"repo":"conf", "from":hidden.entity_id, "to":hidden.entity_id}),
        json!({"repo":"conf", "from":seed.a2, "to":hidden.entity_id}),
    ] {
        assert!(reader.call(&identity, "path", args).await.unwrap()["data"].is_null());
    }
    let neighbors = reader
        .call(&identity, "neighbors", json!({"repo":"conf", "id":seed.a2}))
        .await
        .unwrap();
    assert_eq!(neighbors["data"]["items"], json!([]));
    let export = reader
        .call(&identity, "export", json!({"repo":"conf", "kind":"nodes"}))
        .await
        .unwrap();
    assert_eq!(export["data"]["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn foreign_evidence_blocks_even_relationship_existence() {
    for (snapshot, file) in [("foreign-snapshot", "f.rs"), ("s1", "private/key.rs")] {
        let mut seed = cited_seed().await;
        let mut rows = seed
            .reader
            .evidence_for(&seed.builds.0, &seed.rel1)
            .await
            .unwrap();
        let citation = rows[0].citation.as_mut().unwrap();
        citation.snapshot = snapshot.into();
        citation.file = file.into();
        seed.reader.attach_evidence(&seed.rel1, &rows);
        let mut view = view(&seed.builds.0, &["s1"]);
        view.excluded_paths.push("private".into());
        let identity = view.identity.clone();
        let err = ScopedReader::admit(seed.reader, view, &identity)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code(), "evidence_scope_unavailable");
    }
}

#[tokio::test]
async fn budgets_fail_explicitly_instead_of_silent_truncation() {
    let seed = cited_seed().await;
    let mut grant = view(&seed.builds.0, &["s1"]);
    grant.budgets.max_nodes = 1;
    let identity = grant.identity.clone();
    assert_eq!(
        ScopedReader::admit(seed.reader, grant, &identity)
            .await
            .err()
            .unwrap()
            .code(),
        "budget_exceeded"
    );

    let seed = cited_seed().await;
    let mut grant = view(&seed.builds.0, &["s1"]);
    grant.budgets.max_calls = 2;
    grant.budgets.max_visits = 1;
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    assert_eq!(
        reader
            .call(
                &identity,
                "path",
                json!({"repo":"conf", "from":seed.a1, "to":seed.b1})
            )
            .await
            .unwrap_err()
            .code(),
        "budget_exceeded"
    );
    assert!(
        reader
            .call(&identity, "status", json!({"repo":"conf"}))
            .await
            .is_ok()
    );
    assert_eq!(
        reader
            .call(&identity, "status", json!({"repo":"conf"}))
            .await
            .unwrap_err()
            .code(),
        "budget_exceeded"
    );
}

#[tokio::test]
async fn strict_argument_types_and_limits_are_enforced() {
    let seed = cited_seed().await;
    let grant = view(&seed.builds.0, &["s1"]);
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    for limit in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("2"),
        json!(201),
        json!(null),
    ] {
        assert_eq!(
            reader
                .call(
                    &identity,
                    "search",
                    json!({"repo":"conf", "query":"Alpha", "limit":limit})
                )
                .await
                .unwrap_err()
                .code(),
            "invalid_arguments"
        );
    }
    for args in [
        json!({"repo":"conf", "query":""}),
        json!({"repo":"conf", "query":"a", "identity":{}}),
        json!({"repo":"conf", "query":"a", "operation":"export"}),
        json!([]),
    ] {
        assert!(reader.call(&identity, "search", args).await.is_err());
    }
    assert!(
        reader
            .call(
                &identity,
                "neighbors",
                json!({"repo":"conf", "id":seed.a1, "rel":"arbitrary"})
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn every_identity_dimension_is_required_at_admission() {
    let seed = cited_seed().await;
    let grant = view(&seed.builds.0, &["s1"]);
    let now = grant.issued_at;
    for field in [
        "company", "project", "agent", "task", "run", "host", "server",
    ] {
        let mut value = serde_json::to_value(&grant.identity).unwrap();
        value[field] = json!("other");
        let other = serde_json::from_value(value).unwrap();
        assert_eq!(
            grant.validate_at(&other, now).unwrap_err().code(),
            "identity_mismatch"
        );
    }
    assert_eq!(
        grant
            .validate_at(&grant.identity, grant.expires_at)
            .unwrap_err()
            .code(),
        "expired"
    );
    assert_eq!(
        grant
            .validate_at(&grant.identity, now - 1)
            .unwrap_err()
            .code(),
        "expired"
    );
    for paths in [
        vec![],
        vec!["../private".into()],
        vec!["/absolute".into()],
        vec!["src/../secret".into()],
    ] {
        let mut invalid = grant.clone();
        invalid.source_paths = paths;
        assert_eq!(
            invalid
                .validate_at(&grant.identity, now)
                .unwrap_err()
                .code(),
            "invalid_read_view"
        );
    }
}

#[tokio::test]
async fn complete_result_byte_budget_includes_provenance() {
    let seed = cited_seed().await;
    let mut grant = view(&seed.builds.0, &["s1"]);
    grant.budgets.max_response_bytes = 1024;
    for field in [
        &mut grant.identity.company,
        &mut grant.identity.project,
        &mut grant.identity.agent,
        &mut grant.identity.task,
        &mut grant.identity.run,
        &mut grant.identity.host,
        &mut grant.identity.server,
    ] {
        *field = "a".repeat(256);
    }
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    assert_eq!(
        reader
            .call(&identity, "lookup", json!({"repo":"conf", "id": "missing"}))
            .await
            .unwrap_err()
            .code(),
        "budget_exceeded"
    );
}

async fn send<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, value: serde_json::Value) {
    use tokio::io::AsyncWriteExt;
    writer
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}

async fn receive<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> serde_json::Value {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn mcp_advertises_only_granted_tools_and_keeps_run_isolation() {
    use tokio::io::{AsyncWriteExt, BufReader};
    let seed = cited_seed().await;
    let mut grant = view(&seed.builds.0, &["s1"]);
    grant.operations = [chaosbox::read_view::Operation::Lookup].into();
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    let (client, server) = tokio::io::duplex(8192);
    let (input, output) = tokio::io::split(server);
    let task = tokio::spawn(chaosbox::read_view::mcp::serve(reader, input, output));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    send(&mut write, json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"protocolVersion":"2025-06-18"}})).await;
    assert_eq!(
        receive(&mut read).await["result"]["serverInfo"]["name"],
        "chaosbox-read-view"
    );
    send(
        &mut write,
        json!({"jsonrpc":"2.0", "id":2, "method":"tools/list"}),
    )
    .await;
    let listing = receive(&mut read).await;
    assert_eq!(listing["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(listing["result"]["tools"][0]["name"], "lookup");
    assert!(listing["result"].get("nextCursor").is_none());
    send(&mut write, json!({"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":{"name":"export", "arguments":{"repo":"conf", "kind":"nodes"}}})).await;
    assert_eq!(receive(&mut read).await["result"]["isError"], true);
    send(&mut write, json!({"jsonrpc":"2.0", "id":4, "method":"tools/call", "params":{"name":"lookup", "arguments":{"repo":"conf", "id":seed.a1}}})).await;
    let response = receive(&mut read).await;
    let payload: serde_json::Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["read_view"]["identity"]["run"], identity.run);
    assert_eq!(payload["data"]["entity_id"], seed.a1);
    write.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn mcp_cancellation_is_scoped_and_idle_revocation_closes_the_pipe() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let seed = cited_seed().await;
    let grant = view(&seed.builds.0, &["s1"]);
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    let revoker = reader.revoker();
    let (client, server) = tokio::io::duplex(8192);
    let (input, output) = tokio::io::split(server);
    let task = tokio::spawn(chaosbox::read_view::mcp::serve(reader, input, output));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    send(
        &mut write,
        json!({"jsonrpc":"2.0", "id":1, "method":"initialize"}),
    )
    .await;
    receive(&mut read).await;
    let call = json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":"status", "arguments":{"repo":"conf"}}});
    let cancel =
        json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":2}});
    write
        .write_all(format!("{call}\n{cancel}\n").as_bytes())
        .await
        .unwrap();
    let response = receive(&mut read).await;
    assert_eq!(response["id"], 2);
    assert_eq!(response["error"]["code"], -32800);
    send(&mut write, json!({"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":{"name":"status", "arguments":{"repo":"conf"}}})).await;
    assert!(receive(&mut read).await["result"]["isError"].is_null());
    revoker.revoke();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut line = String::new();
    assert_eq!(read.read_line(&mut line).await.unwrap(), 0);
}

#[tokio::test]
async fn mcp_rejects_oversized_frames_with_bounded_buffers() {
    use tokio::io::AsyncWriteExt;
    let seed = cited_seed().await;
    let grant = view(&seed.builds.0, &["s1"]);
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let (input, output) = tokio::io::split(server);
    let task = tokio::spawn(chaosbox::read_view::mcp::serve(reader, input, output));
    client.write_all(&vec![b'x'; 32 * 1024]).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropping_the_server_future_settles_its_input_task() {
    use tokio::io::{AsyncWriteExt, BufReader};
    let seed = cited_seed().await;
    let grant = view(&seed.builds.0, &["s1"]);
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    let (client, server) = tokio::io::duplex(8192);
    let (input, output) = tokio::io::split(server);
    let task = tokio::spawn(chaosbox::read_view::mcp::serve(reader, input, output));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    send(
        &mut write,
        json!({"jsonrpc":"2.0", "id":1, "method":"initialize"}),
    )
    .await;
    receive(&mut read).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::task::yield_now().await;
    // The input half must be dropped, not left in a detached Tokio task that
    // still consumes the connector's pipe after its owner has gone away.
    assert!(write.write_all(b"orphan input\n").await.is_err());
}

#[tokio::test]
async fn expiry_is_checked_again_before_serving_a_connection() {
    use tokio::io::AsyncReadExt;
    let seed = cited_seed().await;
    let mut grant = view(&seed.builds.0, &["s1"]);
    grant.expires_at = grant.issued_at + 1;
    let identity = grant.identity.clone();
    let reader = ScopedReader::admit(seed.reader, grant, &identity)
        .await
        .unwrap();
    let (mut client, server) = tokio::io::duplex(8192);
    let (input, output) = tokio::io::split(server);
    let task = tokio::spawn(chaosbox::read_view::mcp::serve(reader, input, output));
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut buf = [0_u8; 1];
    assert_eq!(client.read(&mut buf).await.unwrap(), 0);
}

struct SlowHeader(chaosbox_store::MemoryReader);

#[async_trait::async_trait]
impl GraphQueries for SlowHeader {
    async fn active_build(
        &self,
        repo: &str,
    ) -> Result<Option<chaosbox_store::BuildRow>, chaosbox_store::StoreError> {
        self.0.active_build(repo).await
    }
    async fn published_build(
        &self,
        repo: &str,
        build: &str,
    ) -> Result<Option<chaosbox_store::BuildRow>, chaosbox_store::StoreError> {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        self.0.published_build(repo, build).await
    }
    async fn search_entities(
        &self,
        build: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<chaosbox_store::EntityRow>, chaosbox_store::StoreError> {
        self.0.search_entities(build, like, limit).await
    }
    async fn entity_by_id(
        &self,
        build: &str,
        id: &str,
    ) -> Result<Option<chaosbox_store::EntityRow>, chaosbox_store::StoreError> {
        self.0.entity_by_id(build, id).await
    }
    async fn neighbors_out(
        &self,
        build: &str,
        id: &str,
        rel: Vec<String>,
    ) -> Result<Vec<chaosbox_store::RelRow>, chaosbox_store::StoreError> {
        self.0.neighbors_out(build, id, rel).await
    }
    async fn neighbors_in(
        &self,
        build: &str,
        id: &str,
        rel: Vec<String>,
    ) -> Result<Vec<chaosbox_store::RelRow>, chaosbox_store::StoreError> {
        self.0.neighbors_in(build, id, rel).await
    }
    async fn build_entities(
        &self,
        build: &str,
        limit: i64,
    ) -> Result<Vec<chaosbox_store::EntityRow>, chaosbox_store::StoreError> {
        self.0.build_entities(build, limit).await
    }
    async fn build_relationships(
        &self,
        build: &str,
        limit: i64,
    ) -> Result<Vec<chaosbox_store::RelRow>, chaosbox_store::StoreError> {
        self.0.build_relationships(build, limit).await
    }
    async fn evidence_for(
        &self,
        build: &str,
        rel: &str,
    ) -> Result<Vec<EvidenceRow>, chaosbox_store::StoreError> {
        self.0.evidence_for(build, rel).await
    }
    async fn evidence_for_limited(
        &self,
        build: &str,
        rel: &str,
        limit: i64,
    ) -> Result<Vec<EvidenceRow>, chaosbox_store::StoreError> {
        self.0.evidence_for_limited(build, rel, limit).await
    }
}

#[tokio::test]
async fn whole_admission_deadline_bounds_backend_work() {
    let seed = cited_seed().await;
    let mut grant = view(&seed.builds.0, &["s1"]);
    grant.budgets.timeout_ms = 5;
    let identity = grant.identity.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        ScopedReader::admit(SlowHeader(seed.reader), grant, &identity),
    )
    .await
    .unwrap();
    assert_eq!(result.err().unwrap().code(), "deadline_exceeded");
}

#[test]
#[cfg(target_os = "linux")]
fn connector_cli_fails_closed_and_redacts_credential_diagnostics() {
    use std::io::Write;
    let grant = view("build:missing", &["snapshot:missing"]);
    for (document, expected) in [
        (json!({}), "invalid_read_view"),
        (serde_json::to_value(grant).unwrap(), "backend_unavailable"),
    ] {
        let mut admission = tempfile::NamedTempFile::new().unwrap();
        admission
            .write_all(document.to_string().as_bytes())
            .unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
            .args(["reader", "--admission"])
            .arg(admission.path())
            .env(
                "CHAOSBOX_TYPEDB_PASSWORD_FILE",
                "/private-test-identity/never-present/password",
            )
            .env(
                "CHAOSBOX_JEV_API_KEY_FILE",
                "/private-test-identity/never-present/producer",
            )
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("private-test-identity"));
        let error: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
        assert_eq!(error["blocked"], true);
        assert_eq!(error["code"], expected);
    }
}
