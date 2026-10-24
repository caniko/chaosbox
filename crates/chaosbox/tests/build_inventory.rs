//! Retained-build inspection is bounded, read-only, and never prints connection secrets.
#[path = "../../chaosbox-pg/tests/support/server.rs"]
mod server;

use serde_json::Value;

fn binary() -> std::ffi::OsString {
    std::env::var_os("CHAOSBOX_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_chaosbox").into())
}

#[tokio::test]
async fn inventory_validates_before_credentials() {
    for args in [
        vec!["--repo", ""],
        vec!["--repo", "demo", "--limit", "0"],
        vec!["--repo", "demo", "--limit", "51"],
        vec!["--repo", "demo", "--after", ""],
        vec!["--repo", "demo", "--after", "build:\ncontrol"],
    ] {
        let output = tokio::process::Command::new(binary())
            .args(["db", "inventory"])
            .args(args)
            .env_remove("CHAOSBOX_DATABASE_URL")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        let report: Value =
            serde_json::from_slice(&output.stdout).expect("structured inventory error");
        assert_eq!(report["status"], "error");
        assert!(report["detail"]["reason"]
            .as_str()
            .unwrap()
            .contains("invalid build inventory"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("DATABASE_URL"));
    }
}

async fn page(url: &str, repo: &str, limit: usize, after: Option<&str>) -> Value {
    let mut command = tokio::process::Command::new(binary());
    command.args([
        "db",
        "inventory",
        "--repo",
        repo,
        "--limit",
        &limit.to_string(),
    ]);
    if let Some(after) = after {
        command.args(["--after", after]);
    }
    let output = command
        .env("CHAOSBOX_DATABASE_URL", url)
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}; {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["contract_version"], 2);
    assert_eq!(report["operation"], "db inventory");
    assert_eq!(report["status"], "ready");
    assert_eq!(report["detail"]["contract"], "chaosbox.build-inventory.v1");
    report["detail"]["page"].clone()
}

#[tokio::test]
async fn inventory_deadline_includes_stalled_acquisition_without_exposing_credentials() {
    // Accepts TCP but never answers the startup handshake.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let started = std::time::Instant::now();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(binary())
            .args(["db", "inventory", "--repo", "demo"])
            .env(
                "CHAOSBOX_DATABASE_URL",
                format!(
                    "host=127.0.0.1 port={} user=reader password=synthetic-inventory-password dbname=chaosbox",
                    listener.local_addr().unwrap().port()
                ),
            )
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["detail"]["reason"],
        "build inventory deadline exceeded"
    );
    assert!(started.elapsed() >= std::time::Duration::from_secs(9));
    for bytes in [output.stdout, output.stderr] {
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-inventory-password"));
    }
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_inventory_enumerates_history_without_confusing_status_and_pointer() {
    use chaosbox_core::{
        Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SnapshotFile,
        SourceSpan,
    };
    use chaosbox_store::Store;
    use chaosbox_pg::{store::PgStore, PgConfig};
    use serde_json::json;
    let server = server::Server::start();
    let url = server.url();
    let config = PgConfig::new(url.clone()).unwrap();
    let mut store = PgStore::new(config);
    store.migrate().await.unwrap();
    store
        .ensure_snapshot_files(
            "s-inventory",
            "inventory",
            &[SnapshotFile {
                snapshot: "s-inventory".into(),
                path: "f.rs".into(),
                sha256: "synthetic".into(),
                bytes: 1,
            }],
        )
        .await
        .unwrap();
    let nodes = ["a", "b"].map(|name| {
        Entity::new(
            EntityKind::Symbol,
            "inventory",
            "s-inventory",
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    });
    let mut old = GraphBuild::new("inventory", vec!["s-inventory".into()], 1);
    for node in &nodes {
        old.add_node(node.clone()).unwrap();
    }
    old.add_edge(Relation::new(
        RelationType::Calls,
        &nodes[0].id,
        &nodes[1].id,
        RelationScope::File,
        &old.id,
    ))
    .unwrap();
    store.publish(old.clone(), None).await.unwrap();
    let mut current = GraphBuild::new("inventory", old.snapshot_ids.clone(), 2);
    current.predecessor = Some(old.id.clone());
    current.add_node(nodes[0].clone()).unwrap();
    store
        .publish(current.clone(), Some(old.id.clone()))
        .await
        .unwrap();
    let foreign = GraphBuild::new("foreign", vec![], 1);
    store.publish(foreign.clone(), None).await.unwrap();
    let mut expected = vec![
        json!({"build_id":old.id,"generation":1,"status":"active","is_current":false,"predecessor":null,"node_memberships":2,"edge_memberships":1,"sealed_memberships":1}),
        json!({"build_id":current.id,"generation":2,"status":"active","is_current":true,"predecessor":old.id,"node_memberships":1,"edge_memberships":0,"sealed_memberships":0}),
    ];
    expected.sort_by(|a, b| a["build_id"].as_str().cmp(&b["build_id"].as_str()));
    let whole = page(&url, "inventory", 50, None).await;
    assert_eq!(
        whole,
        json!({"repo":"inventory","current_build":current.id,"builds":expected,"complete":true,"next_after":null,"physical_bytes":null})
    );
    let first = page(&url, "inventory", 1, None).await;
    assert_eq!(first["complete"], false);
    assert_eq!(first["builds"], json!([expected[0]]));
    assert_eq!(first["next_after"], expected[0]["build_id"]);
    let last = page(&url, "inventory", 1, first["next_after"].as_str()).await;
    assert_eq!(last["builds"], json!([expected[1]]));
    assert_eq!(last["complete"], true);
    assert_eq!(last["next_after"], Value::Null);
    for repo in ["absent", "inventory\"; $b isa graph-build; #"] {
        let empty = page(&url, repo, 20, None).await;
        assert_eq!(empty["builds"], json!([]));
        assert_eq!(empty["current_build"], Value::Null);
    }
    assert_eq!(
        page(&url, "inventory", 20, Some(&foreign.id)).await["current_build"],
        current.id
    );
    assert_eq!(page(&url, "inventory", 50, None).await, whole);
    eprintln!(
        "{}",
        json!({"case":"retained-build-inventory","page":whole,"paged_builds":2,"physical_bytes_measured":false})
    );
}
