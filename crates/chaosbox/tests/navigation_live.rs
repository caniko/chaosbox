//! Opt-in read-only qualification against one explicitly selected live build.

use chaosbox_store::GraphQueries;
use chaosbox_pg::{reader::PgReader, PgConfig};

/// Exercise the selected executable, not whichever launcher happens to be on PATH.
async fn native(args: &[&str], requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let binary = std::env::var_os("CHAOSBOX_TEST_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_chaosbox").into());
    let mut child = tokio::process::Command::new(binary)
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
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], request["id"]);
            replies.push(response);
        }
    }
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("native navigation exceeded 60 seconds")
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

#[tokio::test]
#[ignore = "read-only recorded Canix case; requires CHAOSBOX_DATABASE_URL and CHAOSBOX_NAVIGATION_BUILD"]
async fn live_context_cli_and_mcp_share_bounded_source_graph() {
    use serde_json::json;

    let expected = std::env::var("CHAOSBOX_NAVIGATION_BUILD").unwrap();
    let packet = native(
        &[
            "query",
            "context",
            "RuntimeManifest",
            "--repo",
            "canix",
            "--depth",
            "2",
            "--max-nodes",
            "10",
            "--max-chars",
            "6000",
        ],
        &[],
    )
    .await
    .remove(0);
    assert_eq!(packet["build_id"], expected);
    assert_eq!(packet["algorithm"], "bounded-lexical-bfs-v3");
    assert_eq!(packet["exhaustive"], false);
    assert_eq!(packet["truncated"], true);
    assert!(packet.to_string().chars().count() <= 6000);

    let nodes = packet["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty() && nodes.len() <= 10);
    assert!(nodes.iter().any(|n| n["label"] == "RuntimeManifest"));
    let mut reader = PgReader::new(PgConfig::from_env().unwrap());
    reader.connect().await.unwrap();
    let header = reader.active_build("canix").await.unwrap().unwrap();
    assert_eq!(
        header.build_id, expected,
        "active build moved during qualification"
    );
    assert_eq!(packet["generation"], header.generation);
    assert_eq!(packet["snapshots"], json!(header.snapshots));
    for node in nodes {
        let entity = reader
            .entity_by_id(&expected, node["id"].as_str().unwrap())
            .await
            .unwrap()
            .expect("context node must belong to the pinned build");
        assert_eq!(node["source_file"], entity.file);
        assert_eq!(node["snapshot"], entity.snapshot);
        assert!(
            entity.span.is_some(),
            "recorded Canix nodes must be source-cited"
        );
        assert_eq!(node["span"], json!(entity.span));
    }
    for edge in packet["links"].as_array().unwrap() {
        for endpoint in ["source", "target"] {
            assert!(nodes.iter().any(|node| node["id"] == edge[endpoint]));
        }
    }

    let responses = native(&["mcp"], &[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"navigation-regression","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"context","arguments":{"repo":"canix","query":"RuntimeManifest","depth":2,"max_nodes":10,"max_chars":6000}}}),
    ]).await;
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(responses[1]["id"], 2);
    assert!(responses[1].get("error").is_none(), "{}", responses[1]);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let mcp_packet: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        mcp_packet, packet,
        "CLI and fresh MCP process must share the pinned reader"
    );
}

#[tokio::test]
#[ignore = "requires CHAOSBOX_DATABASE_URL and CHAOSBOX_NAVIGATION_BUILD; performs reads only"]
async fn live_neighborhood_limits_ordering_and_membership() {
    let build = std::env::var("CHAOSBOX_NAVIGATION_BUILD").unwrap();
    let mut reader = PgReader::new(PgConfig::from_env().unwrap());
    reader.connect().await.unwrap();
    let edge = reader
        .build_relationships(&build, 1)
        .await
        .unwrap()
        .remove(0);
    let id = &edge.from_entity.entity_id;
    let keys = |rows: Vec<chaosbox_store::RelRow>| {
        rows.into_iter()
            .map(|row| {
                let neighbor = if row.from_entity.entity_id == *id {
                    row.to_entity.entity_id
                } else {
                    row.from_entity.entity_id
                };
                (neighbor, row.rel_id)
            })
            .collect::<Vec<_>>()
    };
    let rows = keys(reader.adjacent_relationships(&build, id, 3).await.unwrap());
    assert!(!rows.is_empty() && rows.len() <= 3);
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        rows,
        keys(reader.adjacent_relationships(&build, id, 3).await.unwrap())
    );
    assert!(reader
        .adjacent_relationships(&build, id, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(reader
        .adjacent_relationships("build:missing", id, 3)
        .await
        .unwrap()
        .is_empty());
    let entities = reader
        .search_entities(&build, "%chaosbox%", 3)
        .await
        .unwrap();
    assert!(entities.len() <= 3);
    assert!(entities.windows(2).all(|pair| {
        (&pair[0].qualified_name, &pair[0].entity_id)
            < (&pair[1].qualified_name, &pair[1].entity_id)
    }));
}
