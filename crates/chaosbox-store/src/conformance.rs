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
    reader.set_active("conf", &b1.id);
    reader.insert_build(b2.clone());
    reader.set_active("conf", &b2.id);
    reader.attach_evidence(
        &r1.id,
        &[EvidenceRow {
            evidence_id: "ev1".into(),
            class: "extracted".into(),
            supports: true,
            text: "[structural] Alpha -> Beta".into(),
            citation: None,
            producer: None,
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
/// entity lists compare ordered by qualified name. The live PostgreSQL test
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
    assert_eq!(active.snapshots, ["s2"]);
    assert!(r.active_build("missing-repo").await.unwrap().is_none());
    let historical = r.published_build("conf", &builds.0).await.unwrap().unwrap();
    assert_eq!(historical.build_id, builds.0);
    assert_eq!(historical.generation, 1);
    assert_eq!(historical.snapshots, ["s1"]);
    assert!(r
        .published_build("other", &builds.0)
        .await
        .unwrap()
        .is_none());
    assert!(r
        .published_build("conf", "missing")
        .await
        .unwrap()
        .is_none());
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
    // Navigation must bound both directions at the backend, preserving scope.
    for id in [a1, b1] {
        let edges = r.adjacent_relationships(&builds.0, id, 1).await.unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].rel_id, rel1);
        assert!(r
            .adjacent_relationships(&builds.0, id, 0)
            .await
            .unwrap()
            .is_empty());
        assert!(r
            .adjacent_relationships(&builds.1, id, 1)
            .await
            .unwrap()
            .is_empty());
    }
    for (build, generation) in [(&builds.0, 1), (&builds.1, 2)] {
        let stats = r.navigation_summary(build).await.unwrap().unwrap();
        assert_eq!(stats["build_id"], *build);
        assert_eq!(stats["generation"], generation);
        assert_eq!(stats["nodes"], 2);
        assert_eq!(stats["edges"], 1);
        assert_eq!(stats["community_count"], 1);
    }
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
    // Both pinned builds retain their snapshots across prefix/sentinel boundaries.
    for (build, names, snapshot) in [
        (&builds.0, ["Alpha", "Beta"], "s1"),
        (&builds.1, ["Alpha", "Gamma"], "s2"),
    ] {
        for limit in [-1_i64, 0, 1, 2, 3] {
            let started = std::time::Instant::now();
            let nodes = r.build_entities(build, limit).await.unwrap();
            let edges = r.build_relationships(build, limit).await.unwrap();
            let elapsed = started.elapsed();
            let count = usize::try_from(limit.max(0)).unwrap().min(names.len());
            assert_eq!(
                nodes
                    .iter()
                    .map(|n| n.qualified_name.as_str())
                    .collect::<Vec<_>>(),
                names[..count],
                "build {build}, limit {limit}"
            );
            assert!(nodes.iter().all(|n| n.snapshot == snapshot));
            assert_eq!(edges.len(), count.min(1));
            eprintln!(
                "{}",
                serde_json::json!({
                    "case":"membership-boundary", "build":build, "limit":limit,
                    "nodes":nodes.len(), "edges":edges.len(),
                    "projected_json_bytes":serde_json::to_vec(&(&nodes, &edges)).unwrap().len(),
                    "elapsed_us":elapsed.as_micros(),
                })
            );
        }
    }
    assert!(r
        .build_relationships("build:missing", 1)
        .await
        .unwrap()
        .is_empty());
    // Evidence attaches to the relationship within its own build only.
    let ev = r.evidence_for(&builds.0, rel1).await.unwrap();
    assert_eq!(ev.len(), 1);
    assert!(r
        .evidence_for_limited(&builds.0, rel1, 0)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        r.evidence_for_limited(&builds.0, rel1, 1)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(ev[0].evidence_id, "ev1");
    assert!(ev[0].supports);
    assert!(r.evidence_for(&builds.1, rel1).await.unwrap().is_empty());
    assert!(r
        .evidence_for(&builds.0, "rel:missing")
        .await
        .unwrap()
        .is_empty());
}

use chaosbox_core::{Candidate, Decision, Evidence, SnapshotFile};

fn write_entity(repo: &str, snap: &str, file: &str, name: &str) -> Entity {
    Entity::new(
        chaosbox_core::EntityKind::Symbol,
        repo,
        snap,
        file,
        name,
        name,
        chaosbox_core::SourceSpan::point(file, 1, 1, 0),
    )
}

async fn check_evidence_linkage<S: crate::Store>(s: &mut S) {
    use chaosbox_core::{EvidenceClass, SourceSpan};
    // File versions register before evidence may reference them.
    let files = vec![SnapshotFile {
        snapshot: "s1".into(),
        path: "a.rs".into(),
        sha256: "abc".into(),
        bytes: 3,
    }];
    s.ensure_snapshot_files("s1", "r", &files).await.unwrap();
    let ev = Evidence {
        id: "ev1".into(),
        class: EvidenceClass::Extracted,
        supports: true,
        text: "[structural] a".into(),
        span: Some(SourceSpan::point("a.rs", 1, 1, 0)),
        snapshot: "s1".into(),
        source_file_version: "a.rs".into(),
        producer: None,
    };
    s.put_evidence(ev).await.unwrap();
    assert_eq!(s.stats().evidence, 1);
    let bad = Evidence {
        id: "ev2".into(),
        class: EvidenceClass::Ambiguous,
        supports: false,
        text: "x".into(),
        span: None,
        snapshot: "s9".into(),
        source_file_version: "missing.rs".into(),
        producer: None,
    };
    assert!(
        s.put_evidence(bad).await.is_err(),
        "unregistered files never resolve"
    );
}

/// Shared write-path conformance over any [`Store`](crate::Store) impl: file linkage,
/// decision idempotency + Failed-supersedure, and publication guards.
/// Runs against [`MemoryStore`](crate::MemoryStore) and, from live tests,
/// against a freshly migrated durable backend; the suite seeds repo `r`.
pub async fn check_write_conformance<S: crate::Store>(s: &mut S) {
    use chaosbox_core::{DecisionOutcome, EvidenceClass, RelationType};
    // Run + set identity registers before candidates may reference it.
    s.ensure_run("run:1", "r", "s1", "set:1", "catalog:1", "rubric-v1")
        .await
        .unwrap();
    assert_eq!(s.stats().runs, 1);
    // Same inputs re-register idempotently; changed inputs are rejected.
    s.ensure_run("run:1", "r", "s1", "set:1", "catalog:1", "rubric-v1")
        .await
        .unwrap();
    assert!(s
        .ensure_run("run:1", "r", "s1", "set:1", "catalog:2", "rubric-v1")
        .await
        .is_err());
    let cand = Candidate {
        id: "cand:1".into(),
        rel_type: RelationType::Calls,
        from_entity: "ent:a".into(),
        to_entity: "ent:b".into(),
        reason: "structural".into(),
        state_excerpt: String::new(),
    };
    assert!(
        s.put_candidate("set:missing", &cand).await.is_err(),
        "unregistered sets never resolve"
    );
    s.put_candidate("set:1", &cand).await.unwrap();
    s.put_candidate("set:1", &cand).await.unwrap();
    assert_eq!(s.stats().candidates, 1);
    check_evidence_linkage(s).await;
    // Decisions: first write wins, except Failed supersedes once.
    let mk = |id: &str, outcome| Decision {
        id: id.into(),
        candidate_id: "c1".into(),
        question_id: "q1".into(),
        outcome,
        evidence_class: EvidenceClass::Ambiguous,
        model_requested: "jev-1.13.0".into(),
        model_returned: "jev-1.13.0".into(),
        confidence: None,
        probability: None,
        cache_key: "test-cache-key".into(),
        reuse_key: String::new(),
        raw_answer: None,
    };
    s.put_decision(mk("d1", DecisionOutcome::Failed("down".into())))
        .await
        .unwrap();
    s.put_decision(mk("d2", DecisionOutcome::Accepted))
        .await
        .unwrap();
    assert_eq!(s.stats().decisions, 1, "one row per (candidate, question)");
    s.put_decision(mk("d3", DecisionOutcome::Rejected))
        .await
        .unwrap();
    assert_eq!(s.stats().decisions, 1, "accepted outcomes stick");
    // Publication: stale predecessors and older generations rejected.
    let mut b1 = GraphBuild::new("r", vec!["s1".into()], 1);
    b1.add_node(write_entity("r", "s1", "a.rs", "a")).unwrap();
    s.publish(b1.clone(), None).await.unwrap();
    let mut stale = GraphBuild::new("r", vec!["s1".into()], 1);
    stale
        .add_node(write_entity("r", "s1", "b.rs", "b"))
        .unwrap();
    assert!(s
        .publish(stale, Some("wrong-predecessor".into()))
        .await
        .is_err());
    let mut b2 = GraphBuild::new("r", vec!["s2".into()], 2);
    b2.predecessor = Some(b1.id.clone());
    b2.add_node(write_entity("r", "s2", "c.rs", "c")).unwrap();
    assert!(s.publish(b2.clone(), Some(b1.id.clone())).await.is_ok());
    assert_eq!(s.active("r").unwrap().id, b2.id);
    assert_eq!(s.stats().builds, 2);
}
