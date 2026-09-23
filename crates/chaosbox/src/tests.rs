//! Crate unit tests, moved verbatim from the former inline `mod tests` block.

use super::*;
use chaosbox_core::{diff_builds, SourceSpan};
use chaosbox_store::Store as _;

#[test]
fn export_is_deterministic_and_compatible() {
    let mut b = GraphBuild::new("r", vec!["s".into()], 1);
    let e = Entity::new(
        chaosbox_core::EntityKind::Symbol,
        "r",
        "s",
        "a.rs",
        "a",
        "a",
        SourceSpan::point("a.rs", 1, 1, 0),
    );
    b.add_node(e).unwrap();
    let v1 = export_json(&b);
    let v2 = export_json(&b);
    assert_eq!(v1, v2);
    assert!(v1.get("nodes").is_some() && v1.get("links").is_some());
}

#[test]
fn diff_reports_add_remove() {
    let mut a = GraphBuild::new("r", vec!["s1".into()], 1);
    let mut b = GraphBuild::new("r", vec!["s2".into()], 2);
    let e = Entity::new(
        chaosbox_core::EntityKind::Symbol,
        "r",
        "s1",
        "a.rs",
        "a",
        "a",
        SourceSpan::point("a.rs", 1, 1, 0),
    );
    a.add_node(e.clone()).unwrap();
    b.add_node(e.clone()).unwrap();
    let e2 = Entity::new(
        chaosbox_core::EntityKind::Symbol,
        "r",
        "s2",
        "b.rs",
        "b",
        "b",
        SourceSpan::point("b.rs", 1, 1, 0),
    );
    b.add_node(e2.clone()).unwrap();
    let d = diff_builds(&a, &b);
    assert_eq!(d.added_nodes, vec![e2.id]);
    assert!(d.removed_nodes.is_empty());
}

#[test]
fn threshold_change_reuses_decisions() {
    // Same decisions, different materialization => different accepted sets.
    // Identity covers every threshold so raw decisions are reusable.
    let m1 = Materialization {
        accept_noul: 0.95,
        ..Default::default()
    };
    let m2 = Materialization::default();
    assert_ne!(m1.identity("x"), m2.identity("x"));
    let m3 = Materialization {
        accept_score: 2.0,
        ..Default::default()
    };
    assert_ne!(m3.identity("x"), m2.identity("x"));
    let m4 = Materialization {
        accept_confidence: 0.99,
        ..Default::default()
    };
    assert_ne!(m4.identity("x"), m2.identity("x"));
    let m5 = Materialization {
        abstain_confidence: 0.1,
        ..Default::default()
    };
    assert_ne!(m5.identity("x"), m2.identity("x"));
    assert!(m2.validate().is_ok());
    let bad = Materialization {
        abstain_confidence: 0.9,
        accept_confidence: 0.6,
        ..Default::default()
    };
    assert!(
        bad.validate().is_err(),
        "abstain floor above accept floor is incoherent"
    );
}

/// Responder with tunable Choice confidence for abstain tests.
struct ConfResponder {
    confidence: f64,
}

#[async_trait::async_trait]
impl Responder for ConfResponder {
    async fn respond(
        &mut self,
        _state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        let mut answers = BTreeMap::new();
        for id in questions.keys() {
            answers.insert(
                id.clone(),
                Answer::Choice(ChoiceAnswer {
                    choice: "accept".into(),
                    probabilities: BTreeMap::from([
                        ("accept".into(), 0.9),
                        ("reject".into(), 0.05),
                        ("none".into(), 0.05),
                    ]),
                    confidence: self.confidence,
                }),
            );
        }
        Ok(SystemOneResponse {
            model: chaosbox_jev::JEV_MODEL_PINNED.into(),
            answers,
            usage: chaosbox_jev::Usage {
                input_tokens: 1,
                output_tokens: 0,
            },
        })
    }
}

/// Responder that fails every call (transport fault simulation).
struct FailResponder;

#[async_trait::async_trait]
impl Responder for FailResponder {
    async fn respond(
        &mut self,
        _state: serde_json::Value,
        _questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        Err("transport down".into())
    }
}

fn one_candidate() -> (Candidate, BTreeMap<String, Entity>) {
    use chaosbox_core::{EntityKind, RelationType, SourceSpan};
    let span = SourceSpan::point("a.rs", 1, 1, 0);
    let from = Entity::new(EntityKind::Symbol, "r", "s", "a.rs", "a", "a", span.clone());
    let to = Entity::new(EntityKind::Symbol, "r", "s", "a.rs", "b", "b", span);
    let cand = Candidate {
        id: "cand:1".into(),
        rel_type: RelationType::Calls,
        from_entity: from.id.clone(),
        to_entity: to.id.clone(),
        reason: "structural".into(),
        state_excerpt: "a calls b".into(),
    };
    let entities = BTreeMap::from([(from.id.clone(), from), (to.id.clone(), to)]);
    (cand, entities)
}

#[tokio::test]
async fn below_floor_confidence_abstains() {
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let mut store = MemoryStore::new();
    store
        .ensure_snapshot_files(
            "s",
            "r",
            &[chaosbox_core::SnapshotFile {
                snapshot: "s".into(),
                path: "a.rs".into(),
                sha256: "abc".into(),
                bytes: 3,
            }],
        )
        .await
        .unwrap();
    let mut low = ConfResponder { confidence: 0.1 };
    let decided = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut low,
        "jev-1.13.0",
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(decided[0].1.outcome, DecisionOutcome::Abstained);
    assert_eq!(store.stats().decisions, 1, "abstentions persist");
    // Same inputs reuse the stored decision even when the responder would
    // now fail: no re-ask on a cache hit.
    let mut failing = FailResponder;
    let reused = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut failing,
        "jev-1.13.0",
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(reused[0].1.outcome, DecisionOutcome::Abstained);
    // A fresh store re-asks: above the accept floor the answer is accepted.
    let mut fresh = MemoryStore::new();
    fresh
        .ensure_snapshot_files(
            "s",
            "r",
            &[chaosbox_core::SnapshotFile {
                snapshot: "s".into(),
                path: "a.rs".into(),
                sha256: "abc".into(),
                bytes: 3,
            }],
        )
        .await
        .unwrap();
    let mut high = ConfResponder { confidence: 0.95 };
    let decided = Pipeline::<MemoryStore>::decide(
        &[cand],
        &entities,
        &mut high,
        "jev-1.13.0",
        &mat,
        &mut fresh,
    )
    .await
    .unwrap();
    assert_eq!(decided[0].1.outcome, DecisionOutcome::Accepted);
}

#[tokio::test]
async fn responder_faults_become_failed_decisions() {
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let mut store = MemoryStore::new();
    store
        .ensure_snapshot_files(
            "s",
            "r",
            &[chaosbox_core::SnapshotFile {
                snapshot: "s".into(),
                path: "a.rs".into(),
                sha256: "abc".into(),
                bytes: 3,
            }],
        )
        .await
        .unwrap();
    let mut failing = FailResponder;
    let decided = Pipeline::<MemoryStore>::decide(
        &[cand],
        &entities,
        &mut failing,
        "jev-1.13.0",
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(decided.len(), 1, "batch continues past one fault");
    assert!(matches!(decided[0].1.outcome, DecisionOutcome::Failed(_)));
    // The fault text is never copied into evidence.
    assert!(!decided[0].2.text.contains("transport"));
    assert_eq!(store.stats().decisions, 1, "failures persist for retry");
}

#[tokio::test]
async fn failed_refresh_preserves_last_good_build() {
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let snap = Snapshot {
        id: "s".into(),
        repo: "r".into(),
        files: vec![],
        contents: BTreeMap::new(),
    };
    let ext = Extraction {
        entities: entities.values().cloned().collect(),
        explicit_refs: vec![],
    };
    // First publish a good build on one pipeline/store.
    let mut pipe = Pipeline::<MemoryStore>::new();
    ensure_a_rs(&mut pipe.store).await;
    let mut accept = ConfResponder { confidence: 0.95 };
    let good = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut accept,
        "jev-1.13.0",
        &mat,
        &mut pipe.store,
    )
    .await
    .unwrap();
    let build = pipe
        .build_and_publish("r", &snap, &ext, &good, &mat, None)
        .await
        .unwrap();
    let active_before = pipe.store.active("r").map(|b| b.id);
    assert_eq!(active_before, Some(build.id.clone()));
    assert_eq!(pipe.generation, 1);
    // A failed refresh on the same repository must not publish: the
    // active build and generation stay exactly as-is. A new model
    // identity forces re-asking instead of reusing the cached accept.
    let mut failing = FailResponder;
    let bad = Pipeline::<MemoryStore>::decide(
        &[cand],
        &entities,
        &mut failing,
        "jev-9.9.9",
        &mat,
        &mut pipe.store,
    )
    .await
    .unwrap();
    assert!(matches!(bad[0].1.outcome, DecisionOutcome::Failed(_)));
    let err = pipe
        .build_and_publish("r", &snap, &ext, &bad, &mat, Some(build.id.clone()))
        .await
        .expect_err("failed batch must not publish");
    assert!(
        err.to_string().contains("refusing to publish"),
        "unexpected error: {err}"
    );
    assert_eq!(pipe.store.active("r").map(|b| b.id), active_before);
    assert_eq!(pipe.generation, 1, "rejected batch mints no generation");
    let counts = summarize_outcomes(&bad);
    assert_eq!(counts.get("failed").copied().unwrap_or(0), 1);
}

/// Ensure helper for the single-file `one_candidate` fixture.
async fn ensure_a_rs(store: &mut MemoryStore) {
    store
        .ensure_snapshot_files(
            "s",
            "r",
            &[chaosbox_core::SnapshotFile {
                snapshot: "s".into(),
                path: "a.rs".into(),
                sha256: "abc".into(),
                bytes: 3,
            }],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn cache_invalidates_per_axis() {
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let mut store = MemoryStore::new();
    ensure_a_rs(&mut store).await;
    // Baseline: accepted under jev-1.13.0 / rubric-v1.
    let mut accept = ConfResponder { confidence: 0.95 };
    let first = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut accept,
        "jev-1.13.0",
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(first[0].1.outcome, DecisionOutcome::Accepted);
    // Model change invalidates: re-asked (low confidence now abstains),
    // and the stale row is replaced because the key differs.
    let mut low = ConfResponder { confidence: 0.1 };
    let second = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut low,
        "jev-9.9.9",
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(second[0].1.outcome, DecisionOutcome::Abstained);
    // Rubric change invalidates the same way.
    let mat2 = Materialization {
        rubric_version: "rubric-v2".into(),
        ..Default::default()
    };
    let third = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut low,
        "jev-1.13.0",
        &mat2,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(third[0].1.outcome, DecisionOutcome::Abstained);
    // Catalog change (extra candidate) invalidates the whole run.
    let mut extra = cand.clone();
    extra.id = "cand:2".into();
    let fourth = Pipeline::<MemoryStore>::decide(
        &[cand.clone(), extra],
        &entities,
        &mut low,
        "jev-1.13.0",
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(fourth[0].1.outcome, DecisionOutcome::Abstained);
    // Threshold-only change keeps the key: the stored decision is reused
    // (materialization applies current thresholds later, not here).
    // Fresh store so earlier legs haven't replaced the row.
    let mut store2 = MemoryStore::new();
    ensure_a_rs(&mut store2).await;
    let mut accept2 = ConfResponder { confidence: 0.95 };
    let base = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut accept2,
        "jev-1.13.0",
        &mat,
        &mut store2,
    )
    .await
    .unwrap();
    assert_eq!(base[0].1.outcome, DecisionOutcome::Accepted);
    let mat3 = Materialization {
        accept_confidence: 0.99,
        ..Default::default()
    };
    let mut failing = FailResponder;
    let fifth = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &mut failing,
        "jev-1.13.0",
        &mat3,
        &mut store2,
    )
    .await
    .unwrap();
    assert_eq!(
        fifth[0].1.outcome,
        DecisionOutcome::Accepted,
        "threshold change reuses raw decision"
    );
}

#[test]
fn fresh_process_chains_off_the_live_build() {
    assert_eq!(chain_publication(None).unwrap(), (None, 0));
    assert_eq!(
        chain_publication(Some(("b1".into(), 3))).unwrap(),
        (Some("b1".into()), 3)
    );
    assert!(chain_publication(Some(("b1".into(), -1))).is_err());
}

#[tokio::test]
async fn empty_decisions_publish_entities_only() {
    use chaosbox_core::EntityKind;
    use chaosbox_extract::FileVersion;
    let snap = Snapshot {
        id: "snap:x".into(),
        repo: "r".into(),
        files: vec![FileVersion {
            path: "a.rs".into(),
            sha256: "00".into(),
            bytes: 9,
        }],
        contents: BTreeMap::from([("a.rs".into(), "fn a() {}\n".into())]),
    };
    let ext = Extraction {
        entities: vec![Entity::new(
            EntityKind::Symbol,
            "r",
            &snap.id,
            "a.rs",
            "a",
            "a",
            SourceSpan::point("a.rs", 1, 1, 0),
        )],
        explicit_refs: vec![],
    };
    let mut pipe = Pipeline::<MemoryStore>::new();
    let build = pipe
        .build_and_publish("r", &snap, &ext, &[], &Materialization::default(), None)
        .await
        .unwrap();
    assert!(!build.nodes.is_empty(), "entities must publish");
    assert!(build.edges.is_empty(), "no decisions means no relations");
}

#[test]
fn rel_filter_validation_is_loud() {
    assert_eq!(
        validate_rel_filter(Some(vec!["Calls".into()])).unwrap(),
        Some(vec!["calls".into()])
    );
    assert!(validate_rel_filter(None).unwrap().is_none());
    let err = validate_rel_filter(Some(vec!["frobnicate".into()])).unwrap_err();
    assert!(err.to_string().contains("valid:"), "{err}");
}

#[test]
fn search_returns_sorted_top_n() {
    let mut b = GraphBuild::new("r", vec!["s".into()], 1);
    for name in ["zeta", "alpha", "gamma"] {
        b.add_node(Entity::new(
            chaosbox_core::EntityKind::Symbol,
            "r",
            "s",
            "a.rs",
            name,
            name,
            SourceSpan::point("a.rs", 1, 1, 0),
        ))
        .unwrap();
    }
    let hits = search(&b, "a", 2);
    let names: Vec<_> = hits.iter().map(|e| e.name.clone()).collect();
    // Sorted by qualified name first, then truncated: alpha, gamma.
    assert_eq!(names, vec!["alpha".to_owned(), "gamma".to_owned()]);
}

#[test]
fn claim_survives_one_source_removal() {
    let c = Claim {
        id: "c".into(),
        relation_id: "r".into(),
        supporting: vec!["ev1".into(), "ev2".into()],
        contradicting: vec![],
        accepted: true,
    };
    assert!(claim_survives_source_removal(&c, "ev1"));
}

#[tokio::test]
async fn graph_reader_serves_fake_backend() {
    // The Phase 0 seam: the same reader code serves the in-memory fake.
    let seed = chaosbox_store::conformance_seed();
    let reader: GraphReader<chaosbox_store::MemoryReader> =
        GraphReader::pinned(seed.reader, "conf").await.unwrap();
    assert_eq!(reader.build_id, seed.builds.1);
    // Reads are scoped to the pinned build: only the second Alpha shows.
    let hits = reader.search("alpha", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_id, seed.a2);
    assert!(reader.lookup(&seed.a1).await.unwrap().is_none());
    let path = reader.path(&seed.a2, &seed.b1, 4).await.unwrap();
    assert_eq!(path, None, "cross-build entities never connect");
    let explained = reader.explain(&seed.a2).await.unwrap();
    assert_eq!(explained["outgoing"], 1);
    // rel1 belongs to the first build, invisible from the pinned one.
    let ev = reader.evidence(&seed.rel1).await.unwrap();
    assert_eq!(ev["evidence"].as_array().unwrap().len(), 0);
    let v = reader.export().await.unwrap();
    assert_eq!(
        v["build_id"],
        serde_json::Value::String(seed.builds.1.clone())
    );
    let d = reader
        .diff("conf", &seed.builds.0, &seed.builds.1)
        .await
        .unwrap();
    assert!(!d["added_nodes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn path_traversal_budget_is_explicit() {
    let seed = chaosbox_store::conformance_seed();
    let reader: GraphReader<chaosbox_store::MemoryReader> =
        GraphReader::pinned(seed.reader, "conf").await.unwrap();
    let gamma = &reader.search("Gamma", 10).await.unwrap()[0].entity_id;
    let ok = reader.path(&seed.a2, gamma, 4).await.unwrap();
    assert!(ok.is_some(), "a2 references Gamma in the pinned build");
    let err = reader
        .path_with_cap(&seed.a2, gamma, 4, 0)
        .await
        .expect_err("zero visit budget must fail, not hang");
    assert!(
        err.to_string().contains("budget exceeded"),
        "unexpected error: {err}"
    );
}
