//! Semantic retrieval end to end: publication texts, embedding backfill,
//! pgvector nearest neighbours and rank fusion in search/context, with a
//! deterministic loopback embedding server (keyword axes, not a model).

#[path = "../../chaosbox-pg/tests/support/server.rs"]
mod server;

use std::time::Duration;

use chaosbox::{embed::Embedder, GraphReader};
use chaosbox_core::{Entity, EntityKind, GraphBuild, SourceSpan};
use chaosbox_pg::{reader::PgReader, store::PgStore, PgConfig, EMBED_DIMS};
use chaosbox_store::Store;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Inputs mentioning "pointer" embed on axis 0, everything else on axis 1.
async fn mock_embedder() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buf = [0; 8192];
            let body = loop {
                let n = socket.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break body.to_owned();
                    }
                }
            };
            let input: serde_json::Value = serde_json::from_str(&body).unwrap();
            let data: Vec<_> = input["input"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let mut v = vec![0.0_f32; EMBED_DIMS + 3];
                    v[usize::from(!text.as_str().unwrap().contains("pointer"))] = 1.0;
                    serde_json::json!({"index": index, "embedding": v})
                })
                .collect();
            let reply = serde_json::json!({"data": data}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    format!("http://{addr}/v1/embeddings")
}

fn definition(name: &str, text: &str) -> Entity {
    let mut e = Entity::new(
        EntityKind::Definition,
        "sem",
        "s1",
        "a.rs",
        name,
        &format!("a.rs::{name}"),
        SourceSpan::point("a.rs", 1, 1, 0),
    );
    e.text = Some(text.into());
    e
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_semantic_search_finds_entities_without_lexical_overlap() {
    let server = server::Server::start();
    let config = PgConfig::new(server.url()).unwrap();
    let mut store = PgStore::new(config.clone());
    store.migrate().await.unwrap();
    let mut build = GraphBuild::new("sem", vec!["s1".into()], 1);
    build
        .add_node(definition(
            "swing",
            "/// Move the active pointer.\nfn swing() {}",
        ))
        .unwrap();
    build
        .add_node(definition("parse", "fn parse() {}"))
        .unwrap();
    store.publish(build, None).await.unwrap();

    let embedder = Embedder::new(&mock_embedder().await, "mock", Duration::from_secs(5)).unwrap();
    let stored = chaosbox::embed::backfill(
        &mut PgStore::new(config.clone()),
        &embedder,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(stored, 2);

    let pinned = |embedder: Option<Embedder>| {
        let config = config.clone();
        async move {
            let mut reader = PgReader::new(config);
            reader.connect().await.unwrap();
            GraphReader::pinned(reader, "sem")
                .await
                .unwrap()
                .with_embedder(embedder)
        }
    };
    // No name contains "pointer": only the vector can find `swing`.
    let lexical = pinned(None).await;
    assert!(lexical.search("pointer", 5).await.unwrap().is_empty());
    let semantic = pinned(Some(embedder)).await;
    let hits = semantic.search("pointer", 5).await.unwrap();
    assert_eq!(hits[0].name, "swing", "{hits:?}");
    let context = semantic.context("pointer", 1, 5, 12_000).await.unwrap();
    assert_eq!(context["semantic"], true);
    assert_eq!(context["algorithm"], "bounded-hybrid-bfs-v1");
    // An unreachable endpoint degrades to lexical instead of failing.
    let dead = Embedder::new(
        "http://127.0.0.1:1/v1/embeddings",
        "mock",
        Duration::from_millis(200),
    )
    .unwrap();
    let degraded = pinned(Some(dead)).await;
    assert!(degraded.search("pointer", 5).await.unwrap().is_empty());
    let context = degraded.context("pointer", 1, 5, 12_000).await.unwrap();
    assert_eq!(context["semantic"], false);
    assert_eq!(context["algorithm"], "bounded-lexical-bfs-v2");
}
