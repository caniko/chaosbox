//! Shared high-degree fixture for backend pages and ordinary client enumeration.

use chaosbox_core::{Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SourceSpan};
use chaosbox_store::NeighborhoodDirection;
use serde_json::{Value, json};

pub const DEGREE: usize = 1024;
pub const REPO: &str = "neighborhood-pages";

pub struct Fixture {
    pub builds: [GraphBuild; 3],
    pub hub: String,
    pub isolated: String,
    pub foreign: String,
}

pub fn fixture() -> Fixture {
    let entity = |repo, snapshot, name: &str| {
        Entity::new(
            EntityKind::Symbol,
            repo,
            snapshot,
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = entity(REPO, "s-pages", "hub");
    let isolated = entity(REPO, "s-pages", "isolated");
    let first = entity(REPO, "s-pages", "leaf-0");
    let mut old = GraphBuild::new(REPO, vec!["s-pages".into()], 1);
    for node in [&hub, &isolated, &first] {
        old.add_node(node.clone()).unwrap();
    }
    add_edge(&mut old, RelationType::Calls, &hub.id, &first.id);
    let mut current = GraphBuild::new(REPO, vec!["s-pages".into()], 2);
    current.predecessor = Some(old.id.clone());
    current.add_node(hub.clone()).unwrap();
    current.add_node(isolated.clone()).unwrap();
    for index in 0..DEGREE {
        let leaf = entity(REPO, "s-pages", &format!("leaf-{index}"));
        current.add_node(leaf.clone()).unwrap();
        add_edge(&mut current, RelationType::Calls, &hub.id, &leaf.id);
        add_edge(&mut current, RelationType::Calls, &leaf.id, &hub.id);
    }
    add_edge(&mut current, RelationType::Calls, &hub.id, &hub.id);
    add_edge(&mut current, RelationType::Defines, &hub.id, &hub.id);
    for direction in [
        NeighborhoodDirection::Outgoing,
        NeighborhoodDirection::Incoming,
    ] {
        add_sparse_edges(&mut current, &hub.id, direction);
    }
    let foreign = entity("pages-other", "s-other", "hub");
    let mut other = GraphBuild::new("pages-other", vec!["s-other".into()], 1);
    other.add_node(foreign.clone()).unwrap();
    add_edge(&mut other, RelationType::Calls, &foreign.id, &foreign.id);
    Fixture {
        builds: [old, current, other],
        hub: hub.id,
        isolated: isolated.id,
        foreign: foreign.id,
    }
}

fn add_sparse_edges(build: &mut GraphBuild, hub: &str, direction: NeighborhoodDirection) {
    let excluded = build
        .edges
        .values()
        .filter(|edge| anchored(edge, hub, direction) && edge.rel_type != RelationType::References)
        .map(|edge| edge.id.clone())
        .collect::<Vec<_>>();
    let mut candidates = build
        .nodes
        .values()
        .filter(|e| e.name.starts_with("leaf-"))
        .map(|leaf| {
            let (from, to) = match direction {
                NeighborhoodDirection::Outgoing => (hub, leaf.id.as_str()),
                NeighborhoodDirection::Incoming => (leaf.id.as_str(), hub),
            };
            Relation::new(
                RelationType::References,
                from,
                to,
                RelationScope::File,
                &build.id,
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    let mut previous = String::new();
    // A sparse match after each long excluded interval exercises resumed filtering, too.
    for cutoff in [DEGREE / 2, 3 * DEGREE / 4, DEGREE - 32] {
        let edge = candidates
            .iter()
            .find(|edge| edge.id > excluded[cutoff])
            .unwrap()
            .clone();
        assert!(
            excluded
                .iter()
                .filter(|id| **id > previous && **id < edge.id)
                .count()
                > 128
        );
        previous.clone_from(&edge.id);
        build.add_edge(edge).unwrap();
    }
}

fn add_edge(build: &mut GraphBuild, kind: RelationType, from: &str, to: &str) {
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

pub fn anchored(edge: &Relation, id: &str, direction: NeighborhoodDirection) -> bool {
    match direction {
        NeighborhoodDirection::Outgoing => edge.from == id,
        NeighborhoodDirection::Incoming => edge.to == id,
    }
}

pub fn projection(edge: &Relation) -> Value {
    json!({"rel_id":edge.id,"rel_type":chaosbox_core::relation_type_name(&edge.rel_type),
        "from_entity":{"entity_id":edge.from},"to_entity":{"entity_id":edge.to}})
}
