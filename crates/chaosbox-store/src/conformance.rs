//! Shared conformance fixture and assertions for any [`GraphQueries`](crate::GraphQueries) impl.

use chaosbox_core::{Entity, GraphBuild, Relation};

use crate::memory_reader::MemoryReader;
use crate::queries::GraphQueries;
use crate::rows::EvidenceRow;

/// Seed fixture for conformance: two builds of repo `conf`, the second
/// active, sharing a symbol name across snapshots plus one evidence row.
/// Returns the reader plus the ids the suite asserts on.
pub struct ConformanceSeed {
    /// The seeded reader.
    pub reader: MemoryReader,
    /// First-build entity ids (a1 calls b1).
    pub a1: String,
    /// First-build entity ids (a1 calls b1).
    pub b1: String,
    /// First-build `calls` relationship id.
    pub rel1: String,
    /// Second-build entity id sharing `a1`'s name in a new snapshot.
    pub a2: String,
    /// First and second build ids.
    pub builds: (String, String),
}

/// Build the conformance seed (repo `conf`).
#[must_use]
pub fn conformance_seed() -> ConformanceSeed {
    use chaosbox_core::{EntityKind, RelationScope, RelationType, SourceSpan};
    let span = |f: &str| SourceSpan::point(f, 1, 1, 0);
    let ent = |repo: &str, snap: &str, file: &str, name: &str| {
        Entity::new(EntityKind::Symbol, repo, snap, file, name, name, span(file))
    };
    let mut b1 = GraphBuild::new("conf", vec!["s1".into()], 1);
    let a1 = ent("conf", "s1", "f.rs", "Alpha");
    let b1e = ent("conf", "s1", "f.rs", "Beta");
    b1.add_node(a1.clone()).unwrap();
    b1.add_node(b1e.clone()).unwrap();
    let mut r1 = Relation::new(
        RelationType::Calls,
        &a1.id,
        &b1e.id,
        RelationScope::File,
        &b1.id,
    );
    r1.evidence_ids.push("ev1".into());
    b1.add_edge(r1.clone()).unwrap();
    let mut b2 = GraphBuild::new("conf", vec!["s2".into()], 2);
    b2.predecessor = Some(b1.id.clone());
    let a2 = ent("conf", "s2", "f.rs", "Alpha");
    let c2 = ent("conf", "s2", "g.rs", "Gamma");
    b2.add_node(a2.clone()).unwrap();
    b2.add_node(c2.clone()).unwrap();
    let r2 = Relation::new(
        RelationType::References,
        &a2.id,
        &c2.id,
        RelationScope::CrossFile,
        &b2.id,
    );
    b2.add_edge(r2).unwrap();
    let mut reader = MemoryReader::new();
    reader.insert_build(b1.clone());
    reader.insert_build(b2.clone());
    reader.set_active("conf", &b2.id);
    reader.attach_evidence(
        &r1.id,
        vec![EvidenceRow {
            evidence_id: "ev1".into(),
            class: "extracted".into(),
            supports: true,
            text: "[structural] Alpha -> Beta".into(),
        }],
    );
    ConformanceSeed {
        a1: a1.id,
        b1: b1e.id,
        rel1: r1.id.clone(),
        a2: a2.id,
        builds: (b1.id, b2.id),
        reader,
    }
}

/// Conformance assertions over any [`GraphQueries`] impl seeded like
/// [`conformance_seed`]. Every read is scoped to one pinned build: the suite
/// asserts cross-build leakage is impossible, not just that members resolve.
/// Relationship/evidence lists compare as sets (live order is unspecified);
/// entity lists compare ordered by qualified name. The live `TypeDB` test
/// seeds the same fixture through the insert path and calls this function.
// Long shared test helper; splitting it apart is the owning session's call.
#[allow(clippy::too_many_lines)]
pub async fn check_conformance<R: GraphQueries>(
    r: &R,
    a1: &str,
    b1: &str,
    rel1: &str,
    a2: &str,
    builds: &(String, String),
) {
    use std::collections::BTreeSet;
    // Active pointer pins the second build.
    let active = r.active_build("conf").await.unwrap().unwrap();
    assert_eq!(active.build_id, builds.1);
    assert_eq!(active.generation, 2);
    assert!(r.active_build("missing-repo").await.unwrap().is_none());
    // Search is scoped: each build sees only its own Alpha.
    let hits: Vec<_> = r
        .search_entities(&builds.0, "%alpha%", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(hits, vec![a1.to_owned()]);
    let hits: Vec<_> = r
        .search_entities(&builds.1, "%alpha%", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(hits, vec![a2.to_owned()]);
    assert_eq!(
        r.search_entities(&builds.1, "%alpha%", 0)
            .await
            .unwrap()
            .len(),
        0
    );
    // Escaped wildcards match literally, not as patterns (same on the live
    // backend, where backslash is the LIKE escape).
    assert!(r
        .search_entities(&builds.1, "%alp\\_ha%", 10)
        .await
        .unwrap()
        .is_empty());
    // Lookup is scoped: a1 is invisible from the second build.
    assert_eq!(
        r.entity_by_id(&builds.0, a1)
            .await
            .unwrap()
            .unwrap()
            .entity_id,
        a1
    );
    assert!(r.entity_by_id(&builds.1, a1).await.unwrap().is_none());
    assert!(r
        .entity_by_id(&builds.0, "ent:missing")
        .await
        .unwrap()
        .is_none());
    // Neighborhoods honor the type filter and the build scope; empty filter
    // matches nothing.
    let out: BTreeSet<_> = r
        .neighbors_out(&builds.0, a1, vec!["calls".into()])
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.rel_id)
        .collect();
    assert_eq!(out, BTreeSet::from([rel1.to_owned()]));
    assert!(r
        .neighbors_out(&builds.1, a1, vec!["calls".into()])
        .await
        .unwrap()
        .is_empty());
    assert!(r
        .neighbors_out(&builds.0, a1, vec![])
        .await
        .unwrap()
        .is_empty());
    assert!(r
        .neighbors_out(&builds.0, a1, vec!["references".into()])
        .await
        .unwrap()
        .is_empty());
    let inc: BTreeSet<_> = r
        .neighbors_in(&builds.0, b1, vec!["calls".into()])
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.rel_id)
        .collect();
    assert_eq!(inc, BTreeSet::from([rel1.to_owned()]));
    // Build projections are membership-scoped.
    let e1: BTreeSet<_> = r
        .build_entities(&builds.0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(e1, BTreeSet::from([a1.to_owned(), b1.to_owned()]));
    assert!(r
        .build_entities("build:missing", 100)
        .await
        .unwrap()
        .is_empty());
    let r1: BTreeSet<_> = r
        .build_relationships(&builds.0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.rel_id)
        .collect();
    assert_eq!(r1, BTreeSet::from([rel1.to_owned()]));
    // Evidence attaches to the relationship within its own build only.
    let ev = r.evidence_for(&builds.0, rel1).await.unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].evidence_id, "ev1");
    assert!(ev[0].supports);
    assert!(r.evidence_for(&builds.1, rel1).await.unwrap().is_empty());
    assert!(r
        .evidence_for(&builds.0, "rel:missing")
        .await
        .unwrap()
        .is_empty());
}
