//! Regression tests for build-pinned navigation above export limits.

use chaosbox::GraphReader;
use chaosbox_store::MemoryReader;
use chaosbox_core::{Entity, GraphBuild, Relation, EntityKind, RelationType, RelationScope, SourceSpan};
use serde_json::json;

fn span(file: &str) -> SourceSpan {
    SourceSpan::point(file, 1, 1, 0)
}

fn entity(repo: &str, snap: &str, file: &str, name: &str, kind: EntityKind) -> Entity {
    Entity::new(kind, repo, snap, file, name, name, span(file))
}

fn relation(
    rel_type: RelationType,
    from: &str,
    to: &str,
    scope: RelationScope,
    build: &str,
) -> Relation {
    Relation::new(rel_type, from, to, scope, build)
}

/// Build a large graph with >10000 nodes and >20000 edges.
fn large_build() -> GraphBuild {
    let mut build = GraphBuild::new("large-repo", vec!["snap1".into()], 1);
    let mut node_ids = Vec::with_capacity(11000);
    for i in 0..11000 {
        let e = entity(
            "large-repo",
            "snap1",
            &format!("file_{}.rs", i / 100),
            &format!("Node{i}"),
            EntityKind::Symbol,
        );
        node_ids.push(e.id.clone());
        build.add_node(e).unwrap();
    }
    for i in 0..11000 {
        for offset in 1..=2 {
            if i + offset < 11000 {
                let r = relation(
                    RelationType::References,
                    &node_ids[i],
                    &node_ids[i + offset],
                    RelationScope::CrossFile,
                    &build.id,
                );
                build.add_edge(r).unwrap();
            }
        }
    }
    build
}

/// Build a small deterministic graph for context tests.
fn small_build() -> (GraphBuild, Vec<String>) {
    let mut build = GraphBuild::new("small-repo", vec!["snap1".into()], 1);
    let a = entity("small-repo", "snap1", "a.rs", "Alpha", EntityKind::Symbol);
    let b = entity("small-repo", "snap1", "b.rs", "Beta", EntityKind::Symbol);
    let c = entity("small-repo", "snap1", "c.rs", "Gamma", EntityKind::Symbol);
    let d = entity("small-repo", "snap1", "d.rs", "Delta", EntityKind::Symbol);
    let ids = vec![a.id.clone(), b.id.clone(), c.id.clone(), d.id.clone()];
    for e in [a, b, c, d] {
        build.add_node(e).unwrap();
    }
    build
        .add_edge(relation(
            RelationType::References,
            &ids[0],
            &ids[1],
            RelationScope::CrossFile,
            &build.id,
        ))
        .unwrap();
    build
        .add_edge(relation(
            RelationType::References,
            &ids[1],
            &ids[2],
            RelationScope::CrossFile,
            &build.id,
        ))
        .unwrap();
    build
        .add_edge(relation(
            RelationType::References,
            &ids[2],
            &ids[3],
            RelationScope::CrossFile,
            &build.id,
        ))
        .unwrap();
    (build, ids)
}

/// Build a high-degree hub graph.
fn hub_build() -> (GraphBuild, String, Vec<String>) {
    let mut build = GraphBuild::new("hub-repo", vec!["snap1".into()], 1);
    let hub = entity("hub-repo", "snap1", "hub.rs", "Hub", EntityKind::Symbol);
    let hub_id = hub.id.clone();
    build.add_node(hub).unwrap();
    let mut spoke_ids = Vec::new();
    for i in 0..500 {
        let spoke = entity(
            "hub-repo",
            "snap1",
            &format!("spoke_{i}.rs"),
            &format!("Spoke{i}"),
            EntityKind::Symbol,
        );
        spoke_ids.push(spoke.id.clone());
        build.add_node(spoke).unwrap();
        let r = relation(
            RelationType::References,
            &hub_id,
            &spoke_ids[i],
            RelationScope::CrossFile,
            &build.id,
        );
        build.add_edge(r).unwrap();
    }
    (build, hub_id, spoke_ids)
}

async fn pinned_reader(build: GraphBuild, repo: &str) -> GraphReader<MemoryReader> {
    let mut reader = MemoryReader::new();
    let build_id = build.id.clone();
    reader.insert_build(build);
    reader.set_active(repo, &build_id);
    GraphReader::pinned(reader, repo).await.unwrap()
}

#[tokio::test]
async fn export_fails_on_large_graph_over_caps() {
    let build = large_build();
    let reader = pinned_reader(build, "large-repo").await;
    let err = reader.export().await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("truncated") || msg.contains("export"),
        "expected truncation error, got: {msg}"
    );
}

#[tokio::test]
async fn tiny_context_works_with_valid_endpoints_and_limits() {
    let build = large_build();
    assert!(build.nodes.len() > usize::try_from(chaosbox::EXPORT_NODE_CAP).unwrap());
    assert!(build.edges.len() > usize::try_from(chaosbox::EXPORT_EDGE_CAP).unwrap());
    let reader = pinned_reader(build, "large-repo").await;

    let packet = reader.context("Node5000", 1, 3, 2000).await.unwrap();

    assert_eq!(packet["build_id"], reader.build_id);
    assert_eq!(packet["generation"], json!(reader.generation));
    assert_eq!(packet["snapshots"], json!(reader.snapshots));

    let nodes = packet["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty(), "expected nodes in context");
    assert!(nodes.len() <= 3, "node limit respected");
    assert!(packet.to_string().chars().count() <= 2000);

    let links = packet["links"].as_array().unwrap();
    for link in links {
        let src = link["source"].as_str().unwrap();
        let tgt = link["target"].as_str().unwrap();
        assert!(
            nodes.iter().any(|n| n["id"] == src),
            "link source in selected nodes"
        );
        assert!(
            nodes.iter().any(|n| n["id"] == tgt),
            "link target in selected nodes"
        );
    }
    assert!(packet["truncated"].is_boolean());
}

#[tokio::test]
async fn stats_returns_exact_node_edge_counts_and_first_community() {
    let build = large_build();
    let reader = pinned_reader(build, "large-repo").await;

    let stats = reader.stats(5).await.unwrap();

    assert_eq!(stats["nodes"], json!(11000));
    assert_eq!(stats["edges"], json!(21997));
    assert_eq!(stats["build_id"], reader.build_id);
    assert_eq!(stats["generation"], json!(reader.generation));

    let communities = stats["communities"].as_array().unwrap();
    assert!(!communities.is_empty());
    let first = &communities[0];
    assert!(first["id"].is_string());
    assert!(first["size"].as_u64().unwrap() > 0);
    assert!(first["members"].is_array());

    let hubs = stats["hubs"].as_array().unwrap();
    assert!(!hubs.is_empty());
    for hub in hubs {
        assert!(hub["id"].is_string());
        assert!(hub["label"].is_string());
        assert!(hub["degree"].is_number());
    }

    let community_id = first["id"].as_str().unwrap();
    let community = reader.community(community_id, 5).await.unwrap();
    assert_eq!(community["id"], community_id);
    assert!(community["nodes"].is_array());
    assert_eq!(community["build_id"], reader.build_id);
}

#[tokio::test]
async fn small_graph_context_deterministic() {
    let (build, _ids) = small_build();
    let reader1 = pinned_reader(build.clone(), "small-repo").await;
    let reader2 = pinned_reader(build, "small-repo").await;

    let ctx1 = reader1.context("Alpha", 3, 20, 8192).await.unwrap();
    let ctx2 = reader2.context("Alpha", 3, 20, 8192).await.unwrap();

    assert_eq!(
        ctx1, ctx2,
        "context must be deterministic for same graph and query"
    );
}

#[tokio::test]
async fn truncated_highdegree_context_truthful() {
    let (build, hub_id, _spokes) = hub_build();
    let reader = pinned_reader(build, "hub-repo").await;

    let packet = reader.context("Hub", 2, 10, 4096).await.unwrap();

    assert_eq!(
        packet["truncated"],
        json!(true),
        "high-degree context must report truncation"
    );
    let nodes = packet["nodes"].as_array().unwrap();
    assert!(nodes.len() <= 10, "node limit enforced");
    assert!(
        nodes.iter().any(|n| n["id"] == hub_id),
        "hub node must be in result"
    );
}

#[tokio::test]
async fn query_bounds_invalid_rejected() {
    let (build, _ids) = small_build();
    let reader = pinned_reader(build, "small-repo").await;

    assert!(reader.context("", 2, 10, 4096).await.is_err());
    assert!(reader
        .context(&"a".repeat(3000), 2, 10, 4096)
        .await
        .is_err());
    assert!(reader.context("Alpha", 0, 10, 4096).await.is_err());
    assert!(reader.context("Alpha", 7, 10, 4096).await.is_err());
    assert!(reader.context("Alpha", 2, 0, 4096).await.is_err());
    assert!(reader.context("Alpha", 2, 201, 4096).await.is_err());
    assert!(reader.context("Alpha", 2, 10, 100).await.is_err());
    assert!(reader.context("Alpha", 2, 10, 50000).await.is_err());
    assert!(reader.stats(0).await.is_err());
    assert!(reader.stats(201).await.is_err());
    let stats = reader.stats(5).await.unwrap();
    let community_id = stats["communities"][0]["id"].as_str().unwrap();
    assert!(reader.community(community_id, 0).await.is_err());
    assert!(reader.community(community_id, 201).await.is_err());
}

#[tokio::test]
async fn no_cross_build_member_leak() {
    let mut build1 = GraphBuild::new("leak-repo", vec!["snap1".into()], 1);
    let a1 = entity("leak-repo", "snap1", "a.rs", "Alpha", EntityKind::Symbol);
    let b1 = entity("leak-repo", "snap1", "b.rs", "Beta", EntityKind::Symbol);
    build1.add_node(a1.clone()).unwrap();
    build1.add_node(b1.clone()).unwrap();
    build1
        .add_edge(relation(
            RelationType::References,
            &a1.id,
            &b1.id,
            RelationScope::CrossFile,
            &build1.id,
        ))
        .unwrap();

    let mut build2 = GraphBuild::new("leak-repo", vec!["snap2".into()], 2);
    build2.predecessor = Some(build1.id.clone());
    let a2 = entity("leak-repo", "snap2", "a.rs", "Alpha", EntityKind::Symbol);
    let c2 = entity("leak-repo", "snap2", "c.rs", "Gamma", EntityKind::Symbol);
    build2.add_node(a2.clone()).unwrap();
    build2.add_node(c2.clone()).unwrap();
    build2
        .add_edge(relation(
            RelationType::References,
            &a2.id,
            &c2.id,
            RelationScope::CrossFile,
            &build2.id,
        ))
        .unwrap();

    let mut reader_store = MemoryReader::new();
    reader_store.insert_build(build1);
    let build_id = build2.id.clone();
    reader_store.insert_build(build2);
    reader_store.set_active("leak-repo", &build_id);
    let reader = GraphReader::pinned(reader_store, "leak-repo")
        .await
        .unwrap();

    assert_eq!(reader.generation, 2);
    assert_eq!(reader.build_id, build_id);

    let packet = reader.context("Alpha", 2, 20, 4096).await.unwrap();
    let node_ids: Vec<_> = packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert!(
        node_ids.contains(&a2.id.as_str()),
        "build2 Alpha must be present"
    );
    assert!(
        !node_ids.contains(&a1.id.as_str()),
        "build1 Alpha must NOT leak"
    );
    assert!(
        !node_ids.contains(&b1.id.as_str()),
        "build1 Beta must NOT leak"
    );

    let stats = reader.stats(10).await.unwrap();
    assert_eq!(stats["nodes"], json!(2));
    assert_eq!(stats["edges"], json!(1));
    assert_eq!(stats["generation"], json!(2));
}
