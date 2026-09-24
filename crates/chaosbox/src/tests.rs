//! Crate unit tests, moved verbatim from the former inline `mod tests` block.

use super::*;
use chaosbox_core::{diff_builds, SourceSpan};
use chaosbox_store::Store as _;

/// Test policy: whole-tree scope with live inference allowed. Every cache
/// test uses the same policy so reuse/invalidation legs isolate the
/// dimension they claim (model, rubric, catalog) rather than consent.
fn test_policy() -> chaosbox_core::EffectivePolicy {
    chaosbox_core::EffectivePolicy::new(&[], "local", "typesafe-jev").unwrap()
}

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

/// Minimal snapshot matching `one_candidate` entities: repo `r`, id `s`,
/// file `a.rs` pinned at hash `abc` (see `ensure_a_rs`). Reuse binding
/// validation requires the snapshot the hashes came from.
fn test_snapshot() -> Snapshot {
    use chaosbox_extract::FileVersion;
    Snapshot {
        id: "s".into(),
        repo: "r".into(),
        scope: Vec::new(),
        files: vec![FileVersion {
            path: "a.rs".into(),
            sha256: "abc".into(),
            bytes: 3,
        }],
        contents: BTreeMap::from([("a.rs".into(), "a b".into())]),
    }
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
        &test_snapshot(),
        &mut low,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut failing,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut high,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut failing,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        scope: Vec::new(),
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
        &test_snapshot(),
        &mut accept,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut failing,
        "jev-9.9.9",
        &mat,
        &test_policy(),
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

/// A capture-only refresh must republish relations an earlier decisions
/// run already paid for, instead of swinging the active pointer to a
/// node-only build. It must never re-ask: no responder exists here.
#[tokio::test]
async fn cache_only_refresh_republishes_paid_for_relations() {
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let snap = Snapshot {
        id: "s".into(),
        repo: "r".into(),
        scope: Vec::new(),
        files: vec![],
        contents: BTreeMap::new(),
    };
    let ext = Extraction {
        entities: entities.values().cloned().collect(),
        explicit_refs: vec![],
    };
    let mut pipe = Pipeline::<MemoryStore>::new();
    ensure_a_rs(&mut pipe.store).await;
    let mut accept = ConfResponder { confidence: 0.95 };
    let paid = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &test_snapshot(),
        &mut accept,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut pipe.store,
    )
    .await
    .unwrap();
    let build = pipe
        .build_and_publish("r", &snap, &ext, &paid, &mat, None)
        .await
        .unwrap();
    assert!(!build.edges.is_empty(), "paid-for decisions publish edges");

    let reused = decide_cached(
        std::slice::from_ref(&cand),
        &entities,
        &test_snapshot(),
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut pipe.store,
    )
    .await
    .unwrap();
    assert_eq!(reused.len(), 1, "cache hit must be reused, not re-asked");
    assert_eq!(reused[0].1.id, paid[0].1.id, "reuses the stored decision");

    let refreshed = pipe
        .build_and_publish("r", &snap, &ext, &reused, &mat, Some(build.id.clone()))
        .await
        .unwrap();
    assert_eq!(
        refreshed.edges.len(),
        build.edges.len(),
        "capture-only refresh must not drop published relations"
    );
    assert_eq!(pipe.store.active("r").map(|b| b.id), Some(refreshed.id));
}

/// An uncached candidate contributes nothing to a capture-only refresh:
/// it must be skipped rather than answered with an invented inference.
#[tokio::test]
async fn cache_only_refresh_skips_uncached_candidates() {
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let mut store = MemoryStore::new();
    ensure_a_rs(&mut store).await;
    let reused = decide_cached(
        std::slice::from_ref(&cand),
        &entities,
        &test_snapshot(),
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert!(
        reused.is_empty(),
        "no stored decision means nothing to republish"
    );
}

/// The capture-only gate: refresh freely while nothing can be lost, keep
/// the active query view the moment coverage stops being complete.
#[test]
fn capture_only_gate_keeps_the_active_query_view() {
    assert!(
        capture_only_publishable(&Ok(None), 3, 0),
        "no active build at all: a first capture-only publish bootstraps the query view"
    );
    assert!(
        capture_only_publishable(&Ok(Some(false)), 0, 0),
        "a repository that publishes no relations keeps indexing entities"
    );
    assert!(
        capture_only_publishable(&Ok(Some(false)), 3, 0),
        "full cache misses are harmless with no relations on the line"
    );
    assert!(
        capture_only_publishable(&Ok(Some(true)), 3, 3),
        "complete coverage republishes"
    );
    assert!(
        !capture_only_publishable(&Ok(Some(true)), 3, 2),
        "partial coverage must keep the active build"
    );
    assert!(
        !capture_only_publishable(&Ok(Some(true)), 0, 0),
        "assessing nothing is not coverage of a relation-bearing build"
    );
    assert!(
        !capture_only_publishable(&Err("typedb connect: refused".to_owned()), 3, 3),
        "an unreadable active build is never replaced"
    );
}

/// Cache identity carries the source snapshot, so an edited file leaves
/// every stored decision unreusable: a capture-only refresh then covers
/// none of the relations it would republish, which is exactly what the
/// gate refuses to publish over.
#[tokio::test]
async fn source_edit_leaves_capture_only_refresh_without_coverage() {
    use chaosbox_core::{EntityKind, RelationType, SourceSpan};
    let (cand, entities) = one_candidate();
    let mat = Materialization::default();
    let snap = Snapshot {
        id: "s".into(),
        repo: "r".into(),
        scope: Vec::new(),
        files: vec![],
        contents: BTreeMap::new(),
    };
    let ext = Extraction {
        entities: entities.values().cloned().collect(),
        explicit_refs: vec![],
    };
    let mut pipe = Pipeline::<MemoryStore>::new();
    ensure_a_rs(&mut pipe.store).await;
    let paid = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &test_snapshot(),
        &mut ConfResponder { confidence: 0.95 },
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut pipe.store,
    )
    .await
    .unwrap();
    let build = pipe
        .build_and_publish("r", &snap, &ext, &paid, &mat, None)
        .await
        .unwrap();
    assert!(!build.edges.is_empty(), "the paid-for relations publish");
    let active_relations = pipe.store.active("r").map(|b| !b.edges.is_empty());
    assert_eq!(active_relations, Some(true));

    // The same sources, one edit later: new snapshot, so new identities,
    // so no stored decision matches any current candidate.
    let span = SourceSpan::point("a.rs", 1, 1, 0);
    let from = Entity::new(
        EntityKind::Symbol,
        "r",
        "s2",
        "a.rs",
        "a",
        "a",
        span.clone(),
    );
    let to = Entity::new(EntityKind::Symbol, "r", "s2", "a.rs", "b", "b", span);
    let edited = Candidate {
        id: "cand:edited".into(),
        rel_type: RelationType::Calls,
        from_entity: from.id.clone(),
        to_entity: to.id.clone(),
        reason: "structural".into(),
        state_excerpt: "a calls b".into(),
    };
    let edited_entities = BTreeMap::from([(from.id.clone(), from), (to.id.clone(), to)]);
    // Edited endpoint file: different bytes, so the relation-local reuse key
    // misses even though names and excerpt are unchanged.
    let edited_snapshot = {
        use chaosbox_extract::FileVersion;
        Snapshot {
            id: "s2".into(),
            repo: "r".into(),
            scope: Vec::new(),
            files: vec![FileVersion {
                path: "a.rs".into(),
                sha256: "def".into(),
                bytes: 4,
            }],
            contents: BTreeMap::from([("a.rs".into(), "a b edited".into())]),
        }
    };
    let reused = decide_cached(
        std::slice::from_ref(&edited),
        &edited_entities,
        &edited_snapshot,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut pipe.store,
    )
    .await
    .unwrap();
    assert!(
        reused.is_empty(),
        "an edited source invalidates every cached decision"
    );
    assert!(
        !capture_only_publishable(&Ok(active_relations), 1, reused.len()),
        "an uncovered capture-only run must keep the active build"
    );
    assert_eq!(
        pipe.store.active("r").map(|b| b.id),
        Some(build.id),
        "refusing to publish leaves the active pointer untouched"
    );
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
// Per-axis invalidation legs; splitting them apart is the owning session's
// refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
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
        &test_snapshot(),
        &mut accept,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut low,
        "jev-9.9.9",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut low,
        "jev-1.13.0",
        &mat2,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(third[0].1.outcome, DecisionOutcome::Abstained);
    // Catalog change (extra candidate) no longer invalidates the whole run
    // (issue #12): unrelated relations keep their reuse key; only the new
    // candidate spends.
    let mut extra = cand.clone();
    extra.id = "cand:2".into();
    extra.reason = "co-occurrence".into();
    let fourth = Pipeline::<MemoryStore>::decide(
        &[cand.clone(), extra],
        &entities,
        &test_snapshot(),
        &mut low,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(
        fourth[0].1.outcome,
        DecisionOutcome::Accepted,
        "catalog addition must not invalidate unrelated reuse"
    );
    assert_eq!(
        fourth[1].1.outcome,
        DecisionOutcome::Abstained,
        "the new candidate still asks once"
    );
    // Threshold-only change keeps the key: the stored decision is reused
    // (materialization applies current thresholds later, not here).
    // Fresh store so earlier legs haven't replaced the row.
    let mut store2 = MemoryStore::new();
    ensure_a_rs(&mut store2).await;
    let mut accept2 = ConfResponder { confidence: 0.95 };
    let base = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &test_snapshot(),
        &mut accept2,
        "jev-1.13.0",
        &mat,
        &test_policy(),
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
        &test_snapshot(),
        &mut failing,
        "jev-1.13.0",
        &mat3,
        &test_policy(),
        &mut store2,
    )
    .await
    .unwrap();
    assert_eq!(
        fifth[0].1.outcome,
        DecisionOutcome::Accepted,
        "threshold change reuses raw decision"
    );
    // Policy change invalidates: same sources, different consent, so the
    // stored accept must not be reused (low confidence now abstains).
    let other_policy = chaosbox_core::EffectivePolicy::new(&[], "local", "none").unwrap();
    let sixth = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &test_snapshot(),
        &mut low,
        "jev-1.13.0",
        &mat,
        &other_policy,
        &mut store2,
    )
    .await
    .unwrap();
    assert_eq!(
        sixth[0].1.outcome,
        DecisionOutcome::Abstained,
        "privacy/inference change must re-ask, never reuse"
    );
}

/// Policy change with differing answers mints new decision + evidence ids
/// (no stale collision): same snapshot, different consent flips the reuse
/// key, re-asks, replaces the stored decision, and persists matching
/// evidence. Returned and stored rows agree, and evidence ids diverge.
#[tokio::test]
async fn policy_change_mints_new_decision_and_evidence() {
    let (cand, entities) = one_candidate();
    let snap = test_snapshot();
    let mat = Materialization::default();
    let mut store = MemoryStore::new();
    ensure_a_rs(&mut store).await;
    let mut accept = ConfResponder { confidence: 0.95 };
    let first = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut accept,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(first[0].1.outcome, DecisionOutcome::Accepted);
    let old_dec = first[0].1.clone();
    let old_ev = first[0].2.clone();
    // Different consent => different reuse key => re-ask (low conf abstains).
    let other_policy = chaosbox_core::EffectivePolicy::new(&[], "local", "none").unwrap();
    let mut low = ConfResponder { confidence: 0.1 };
    let second = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut low,
        "jev-1.13.0",
        &mat,
        &other_policy,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(second[0].1.outcome, DecisionOutcome::Abstained);
    let new_dec = second[0].1.clone();
    let new_ev = second[0].2.clone();
    assert_ne!(old_dec.reuse_key, new_dec.reuse_key, "policy flips reuse");
    assert_ne!(old_dec.id, new_dec.id, "decision id binds reuse");
    assert_ne!(old_dec.cache_key, new_dec.cache_key);
    assert_ne!(old_ev.id, new_ev.id, "evidence ids never collide");
    assert!(!new_ev.supports, "abstained evidence must not support");
    // Stored readback matches the returned replacement.
    let qid = format!("rel_{}", cand.id);
    let kept = store
        .find_decision(&cand.id, &qid)
        .await
        .unwrap()
        .expect("replacement must persist");
    assert_eq!(kept.id, new_dec.id);
    assert_eq!(kept.outcome, DecisionOutcome::Abstained);
    assert_eq!(kept.reuse_key, new_dec.reuse_key);
    assert_eq!(kept.raw_answer, new_dec.raw_answer);
    // Preflight agrees under the new policy (cached), and the old policy
    // still resolves to its own inference (no cross-consent reuse).
    let pending_new = crate::uncached_decisions(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        "jev-1.13.0",
        &mat,
        &other_policy,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending_new, 0);
    let pending_old = crate::uncached_decisions(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending_old, 0, "old policy inference still cached");
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
        scope: Vec::new(),
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

/// Find the `a.rs::foo -> a.rs::bar` co-occurrence candidate and its
/// endpoints by qualified name (snapshot-independent lookup).
fn find_foo_bar(
    candidates: &[Candidate],
    entities: &BTreeMap<String, Entity>,
) -> (Candidate, Entity, Entity) {
    let by_qualified: BTreeMap<&str, &Entity> = entities
        .values()
        .map(|e| (e.qualified_name.as_str(), e))
        .collect();
    let from = (*by_qualified.get("a.rs::foo").expect("a.rs::foo")).clone();
    let to = (*by_qualified.get("a.rs::bar").expect("a.rs::bar")).clone();
    let cand = candidates
        .iter()
        .find(|c| c.from_entity == from.id && c.to_entity == to.id)
        .expect("foo->bar candidate")
        .clone();
    (cand, from, to)
}

fn write_two_file_repo(root: &std::path::Path, a_rs: &str, unrelated: &str) {
    std::fs::write(root.join("a.rs"), a_rs).unwrap();
    std::fs::write(root.join("unrelated.txt"), unrelated).unwrap();
}

/// Issue #12 identity gate, part 1: an edit to an unrelated file rewrites
/// snapshot/entity/candidate ids but must NOT invalidate the reuse key of
/// an untouched relation.
#[test]
fn reuse_identity_survives_unrelated_edit() {
    let policy = test_policy();
    let mat = Materialization::default();
    let policy_digest = policy.digest();
    let before_dir = tempfile::tempdir().unwrap();
    let after_dir = tempfile::tempdir().unwrap();
    let a_rs = "fn foo() {}\nfn bar() {}\n";
    write_two_file_repo(before_dir.path(), a_rs, "hello\n");
    write_two_file_repo(after_dir.path(), a_rs, "hello, edited\n");
    let before_snap = Snapshot::capture("r", before_dir.path()).unwrap();
    let after_snap = Snapshot::capture("r", after_dir.path()).unwrap();
    assert_ne!(
        before_snap.id, after_snap.id,
        "unrelated edit mints a new snapshot"
    );
    let before_ext = extract_snapshot(&before_snap);
    let after_ext = extract_snapshot(&after_snap);
    let before_cat = build_candidates(&before_ext, 200);
    let after_cat = build_candidates(&after_ext, 200);
    let before_entities: BTreeMap<String, Entity> = before_ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let after_entities: BTreeMap<String, Entity> = after_ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let (before_cand, before_from, before_to) =
        find_foo_bar(&before_cat.candidates, &before_entities);
    let (after_cand, after_from, after_to) = find_foo_bar(&after_cat.candidates, &after_entities);
    assert_ne!(
        before_cand.id, after_cand.id,
        "snapshot-scoped candidate ids rewrite on any edit"
    );
    let before_hashes = file_hashes_for(&before_snap);
    let after_hashes = file_hashes_for(&after_snap);
    let before_questions = questions_for(&before_cand, &before_from, &before_to);
    let after_questions = questions_for(&after_cand, &after_from, &after_to);
    let before_ctx = ReuseContext {
        repo: "r",
        file_hashes: &before_hashes,
        model: chaosbox_jev::JEV_MODEL_PINNED,
        rubric_version: &mat.rubric_version,
        policy_digest: &policy_digest,
    };
    let after_ctx = ReuseContext {
        repo: "r",
        file_hashes: &after_hashes,
        model: chaosbox_jev::JEV_MODEL_PINNED,
        rubric_version: &mat.rubric_version,
        policy_digest: &policy_digest,
    };
    let before_input = reuse_input_for(
        &before_cand,
        &before_from,
        &before_to,
        &before_questions,
        &before_ctx,
        &before_snap.id,
    )
    .unwrap();
    let after_input = reuse_input_for(
        &after_cand,
        &after_from,
        &after_to,
        &after_questions,
        &after_ctx,
        &after_snap.id,
    )
    .unwrap();
    assert_eq!(
        chaosbox_jev::reuse_key(&before_input),
        chaosbox_jev::reuse_key(&after_input),
        "untouched relation keeps its reuse key across an unrelated edit"
    );
}

/// Issue #12 identity gate, part 2: editing an endpoint file without
/// renaming its entities must invalidate the reuse key (file bytes are
/// part of the identity, names alone are not enough).
#[test]
fn reuse_identity_invalidates_on_endpoint_edit() {
    let policy = test_policy();
    let mat = Materialization::default();
    let policy_digest = policy.digest();
    let before_dir = tempfile::tempdir().unwrap();
    let after_dir = tempfile::tempdir().unwrap();
    write_two_file_repo(before_dir.path(), "fn foo() {}\nfn bar() {}\n", "hello\n");
    write_two_file_repo(
        after_dir.path(),
        "fn foo() {\n  // body changed, names kept\n}\nfn bar() {}\n",
        "hello\n",
    );
    let before_snap = Snapshot::capture("r", before_dir.path()).unwrap();
    let after_snap = Snapshot::capture("r", after_dir.path()).unwrap();
    assert_ne!(before_snap.id, after_snap.id);
    let before_ext = extract_snapshot(&before_snap);
    let after_ext = extract_snapshot(&after_snap);
    let before_cat = build_candidates(&before_ext, 200);
    let after_cat = build_candidates(&after_ext, 200);
    let before_entities: BTreeMap<String, Entity> = before_ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let after_entities: BTreeMap<String, Entity> = after_ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let (before_cand, before_from, before_to) =
        find_foo_bar(&before_cat.candidates, &before_entities);
    let (after_cand, after_from, after_to) = find_foo_bar(&after_cat.candidates, &after_entities);
    let before_hashes = file_hashes_for(&before_snap);
    let after_hashes = file_hashes_for(&after_snap);
    assert_ne!(
        before_hashes.get("a.rs"),
        after_hashes.get("a.rs"),
        "endpoint file bytes changed"
    );
    let before_questions = questions_for(&before_cand, &before_from, &before_to);
    let after_questions = questions_for(&after_cand, &after_from, &after_to);
    let before_ctx = ReuseContext {
        repo: "r",
        file_hashes: &before_hashes,
        model: chaosbox_jev::JEV_MODEL_PINNED,
        rubric_version: &mat.rubric_version,
        policy_digest: &policy_digest,
    };
    let after_ctx = ReuseContext {
        repo: "r",
        file_hashes: &after_hashes,
        model: chaosbox_jev::JEV_MODEL_PINNED,
        rubric_version: &mat.rubric_version,
        policy_digest: &policy_digest,
    };
    let before_input = reuse_input_for(
        &before_cand,
        &before_from,
        &before_to,
        &before_questions,
        &before_ctx,
        &before_snap.id,
    )
    .unwrap();
    let after_input = reuse_input_for(
        &after_cand,
        &after_from,
        &after_to,
        &after_questions,
        &after_ctx,
        &after_snap.id,
    )
    .unwrap();
    assert_ne!(
        chaosbox_jev::reuse_key(&before_input),
        chaosbox_jev::reuse_key(&after_input),
        "endpoint body change must re-ask even with identical names"
    );
}

/// Issue #12 Slice 2A: threshold changes rematerialize the same raw answer
/// without re-asking — abstained becomes accepted when the floor drops, and
/// accepted becomes abstained when it rises. The responder that would fail
/// on any spend proves no inference ran. Stored readback must match the
/// returned outcome (no stale rows), preflight must agree (zero uncached),
/// and publication must follow the rematerialized outcome.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn reuse_rematerializes_threshold_change_without_respend() {
    let (cand, entities) = one_candidate();
    let snap = test_snapshot();
    let mut store = MemoryStore::new();
    ensure_a_rs(&mut store).await;
    // Low confidence (0.1) abstains under the default floor (0.4).
    let mut low = ConfResponder { confidence: 0.1 };
    let first = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut low,
        "jev-1.13.0",
        &Materialization::default(),
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(first[0].1.outcome, DecisionOutcome::Abstained);
    assert!(first[0].1.raw_answer.is_some(), "raw must persist");
    // Lower the floor below 0.1: the same raw now accepts, with no spend.
    let lowered = Materialization {
        abstain_confidence: 0.05,
        ..Default::default()
    };
    let mut failing = FailResponder;
    let second = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut failing,
        "jev-1.13.0",
        &lowered,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(
        second[0].1.outcome,
        DecisionOutcome::Accepted,
        "same raw rematerialized under a lower floor"
    );
    // Stored readback must match the returned rematerialization — no stale
    // Abstained row, no stale non-supporting evidence identity.
    let qid = format!("rel_{}", cand.id);
    let kept = store
        .find_decision(&cand.id, &qid)
        .await
        .unwrap()
        .expect("rematerialized decision must persist");
    assert_eq!(kept.id, second[0].1.id, "stored id matches returned");
    assert_eq!(
        kept.outcome,
        DecisionOutcome::Accepted,
        "stored outcome matches rematerialized"
    );
    assert_eq!(kept.cache_key, second[0].1.cache_key);
    assert_eq!(kept.reuse_key, second[0].1.reuse_key);
    assert_eq!(kept.raw_answer, second[0].1.raw_answer);
    assert!(second[0].2.supports);
    assert_eq!(second[0].2.class, second[0].1.evidence_class);
    // Preflight agrees: validated reuse costs nothing.
    let pending = crate::uncached_decisions(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        "jev-1.13.0",
        &lowered,
        &test_policy(),
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 0, "rematerialized reuse must be fully cached");
    // Raise the floor above a previously accepted confidence: accepted
    // becomes abstained, again with no spend.
    let mut store2 = MemoryStore::new();
    ensure_a_rs(&mut store2).await;
    let permissive = Materialization {
        accept_confidence: 0.99,
        abstain_confidence: 0.4,
        ..Default::default()
    };
    let mut high = ConfResponder { confidence: 0.95 };
    let base = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut high,
        "jev-1.13.0",
        &permissive,
        &test_policy(),
        &mut store2,
    )
    .await
    .unwrap();
    assert_eq!(base[0].1.outcome, DecisionOutcome::Accepted);
    let strict = Materialization {
        accept_confidence: 0.99,
        abstain_confidence: 0.96,
        ..Default::default()
    };
    let mut failing2 = FailResponder;
    let remat = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut failing2,
        "jev-1.13.0",
        &strict,
        &test_policy(),
        &mut store2,
    )
    .await
    .unwrap();
    assert_eq!(
        remat[0].1.outcome,
        DecisionOutcome::Abstained,
        "same raw rematerialized under a higher floor"
    );
    // Stored readback follows the higher floor too, and publication drops
    // the relation (Abstained never materializes) instead of publishing a
    // stale Accepted edge.
    let qid2 = format!("rel_{}", cand.id);
    let kept_strict = store2
        .find_decision(&cand.id, &qid2)
        .await
        .unwrap()
        .expect("rematerialized decision must persist");
    assert_eq!(kept_strict.id, remat[0].1.id);
    assert_eq!(kept_strict.outcome, DecisionOutcome::Abstained);
    assert!(!remat[0].2.supports);
    let pending2 = crate::uncached_decisions(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        "jev-1.13.0",
        &strict,
        &test_policy(),
        &store2,
    )
    .await
    .unwrap();
    assert_eq!(pending2, 0, "abstained rematerialization stays cached");
    let ext = Extraction {
        entities: entities.values().cloned().collect(),
        explicit_refs: vec![],
    };
    let mut pipe2 = Pipeline::<MemoryStore>::new();
    pipe2.store = store2;
    let build = pipe2
        .build_and_publish("r", &snap, &ext, &remat, &strict, None)
        .await
        .unwrap();
    assert!(
        build.edges.is_empty(),
        "abstained rematerialization must not publish relations"
    );
}

/// Shared resolver: a recorded `Failed` with the same reuse key blocks reuse
/// even though a valid inference exists — retries always re-ask, then
/// supersede the failure.
#[tokio::test]
async fn failed_retry_blocks_reuse_until_respend() {
    let (cand, entities) = one_candidate();
    let snap = test_snapshot();
    let mat = Materialization::default();
    let mut store = MemoryStore::new();
    ensure_a_rs(&mut store).await;
    let mut accept = ConfResponder { confidence: 0.95 };
    let paid = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut accept,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(paid[0].1.outcome, DecisionOutcome::Accepted);
    let rkey = paid[0].1.reuse_key.clone();
    let qid = format!("rel_{}", cand.id);
    // Inject a failure for the same inputs with a different audit key so it
    // replaces the Accepted row (same inputs failed on retry).
    let mut failed = paid[0].1.clone();
    failed.outcome = DecisionOutcome::Failed("injected fault".into());
    failed.raw_answer = None;
    failed.cache_key = format!("{}:failed-test", paid[0].1.cache_key);
    assert_eq!(failed.reuse_key, rkey);
    store.put_decision(failed).await.unwrap();
    // Preflight and execution agree: uncached, and decide spends.
    let pending = crate::uncached_decisions(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 1, "Failed with same reuse key must re-ask");
    let mut retry = ConfResponder { confidence: 0.95 };
    let second = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut retry,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(second[0].1.outcome, DecisionOutcome::Accepted);
    let stored = store
        .find_decision(&cand.id, &qid)
        .await
        .unwrap()
        .expect("retry must supersede the failure");
    assert_eq!(stored.outcome, DecisionOutcome::Accepted);
}

/// Shared resolver: corrupt inferences fail closed — wrong requested model,
/// empty returned model, and invalid raws are `Err`, never silent reuse and
/// never spend-every-run misses against first-write-wins poison.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn corrupt_inferences_fail_closed() {
    use chaosbox_core::{InferenceRecord, RawAnswer};
    let (cand, entities) = one_candidate();
    let snap = test_snapshot();
    let mat = Materialization::default();
    // Pay once to learn the reuse key for these inputs.
    let mut probe = MemoryStore::new();
    ensure_a_rs(&mut probe).await;
    let mut accept = ConfResponder { confidence: 0.95 };
    let paid = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&cand),
        &entities,
        &snap,
        &mut accept,
        "jev-1.13.0",
        &mat,
        &test_policy(),
        &mut probe,
    )
    .await
    .unwrap();
    let rkey = paid[0].1.reuse_key.clone();
    let valid_raw = paid[0].1.raw_answer.clone().expect("paid has raw");
    let mk_store = |rec: InferenceRecord| async move {
        let mut s = MemoryStore::new();
        ensure_a_rs(&mut s).await;
        s.put_inference(rec).await.unwrap();
        s
    };
    // Wrong requested model.
    let wrong_model = InferenceRecord {
        reuse_key: rkey.clone(),
        raw: valid_raw.clone(),
        model_requested: "wrong-model".into(),
        model_returned: "jev-1.13.0".into(),
    };
    let s = mk_store(wrong_model).await;
    assert!(
        crate::uncached_decisions(
            std::slice::from_ref(&cand),
            &entities,
            &snap,
            "jev-1.13.0",
            &mat,
            &test_policy(),
            &s,
        )
        .await
        .is_err(),
        "model mismatch must fail closed"
    );
    // Empty returned model.
    let empty_returned = InferenceRecord {
        reuse_key: rkey.clone(),
        raw: valid_raw.clone(),
        model_requested: "jev-1.13.0".into(),
        model_returned: String::new(),
    };
    let s = mk_store(empty_returned).await;
    assert!(
        crate::uncached_decisions(
            std::slice::from_ref(&cand),
            &entities,
            &snap,
            "jev-1.13.0",
            &mat,
            &test_policy(),
            &s,
        )
        .await
        .is_err(),
        "empty returned model must fail closed"
    );
    // Invalid raw: distribution does not sum to one.
    let bad_raw = RawAnswer::Choice {
        choice: "accept".into(),
        probabilities: std::collections::BTreeMap::from([
            ("accept".into(), 0.1),
            ("reject".into(), 0.1),
            ("none".into(), 0.1),
        ]),
        confidence: 0.95,
    };
    let bad = InferenceRecord {
        reuse_key: rkey.clone(),
        raw: bad_raw,
        model_requested: "jev-1.13.0".into(),
        model_returned: "jev-1.13.0".into(),
    };
    let mut s = mk_store(bad).await;
    assert!(
        crate::uncached_decisions(
            std::slice::from_ref(&cand),
            &entities,
            &snap,
            "jev-1.13.0",
            &mat,
            &test_policy(),
            &s,
        )
        .await
        .is_err(),
        "invalid raw must fail closed"
    );
    // Decide fails closed too (never spends on corruption).
    let mut failing = FailResponder;
    assert!(
        Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &snap,
            &mut failing,
            "jev-1.13.0",
            &mat,
            &test_policy(),
            &mut s,
        )
        .await
        .is_err(),
        "decide must fail closed on corrupt inference"
    );
}

/// Issue #12 Slice 2B: cross-snapshot reuse — an unrelated-file edit keeps
/// eligible reuse (zero spend on the untouched relation), an endpoint edit
/// misses, and a removed candidate never reappears.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cross_snapshot_reuse_survives_unrelated_edit() {
    let policy = test_policy();
    let mat = Materialization::default();
    let before_dir = tempfile::tempdir().unwrap();
    let after_dir = tempfile::tempdir().unwrap();
    let a_rs = "fn foo() {}\nfn bar() {}\n";
    write_two_file_repo(before_dir.path(), a_rs, "hello\n");
    write_two_file_repo(after_dir.path(), a_rs, "hello, edited\n");
    let before_snap = Snapshot::capture("r", before_dir.path()).unwrap();
    let after_snap = Snapshot::capture("r", after_dir.path()).unwrap();
    let before_ext = extract_snapshot(&before_snap);
    let after_ext = extract_snapshot(&after_snap);
    let before_cat = build_candidates(&before_ext, 200);
    let after_cat = build_candidates(&after_ext, 200);
    let before_entities: BTreeMap<String, Entity> = before_ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let after_entities: BTreeMap<String, Entity> = after_ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let (before_cand, _, _) = find_foo_bar(&before_cat.candidates, &before_entities);
    let (after_cand, _, _) = find_foo_bar(&after_cat.candidates, &after_entities);
    let mut store = MemoryStore::new();
    store
        .ensure_snapshot_files(&before_snap.id, "r", &before_snap.snapshot_files())
        .await
        .unwrap();
    store
        .ensure_snapshot_files(&after_snap.id, "r", &after_snap.snapshot_files())
        .await
        .unwrap();
    // Pay once on the before snapshot.
    let mut accept = ConfResponder { confidence: 0.95 };
    let paid = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&before_cand),
        &before_entities,
        &before_snap,
        &mut accept,
        "jev-1.13.0",
        &mat,
        &policy,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(paid[0].1.outcome, DecisionOutcome::Accepted);
    // Identical rerun spends nothing.
    let pending = uncached_decisions(
        std::slice::from_ref(&before_cand),
        &before_entities,
        &before_snap,
        "jev-1.13.0",
        &mat,
        &policy,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 0, "identical rerun must be fully cached");
    // Unrelated edit: same relation, new ids, still zero spend.
    let pending_after = uncached_decisions(
        std::slice::from_ref(&after_cand),
        &after_entities,
        &after_snap,
        "jev-1.13.0",
        &mat,
        &policy,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(
        pending_after, 0,
        "untouched relation survives an unrelated edit"
    );
    let mut failing = FailResponder;
    let reused = Pipeline::<MemoryStore>::decide(
        std::slice::from_ref(&after_cand),
        &after_entities,
        &after_snap,
        &mut failing,
        "jev-1.13.0",
        &mat,
        &policy,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(reused[0].1.outcome, DecisionOutcome::Accepted);
    assert_ne!(
        reused[0].1.id, paid[0].1.id,
        "rebound to current snapshot ids, not byte-reused"
    );
    assert_eq!(
        reused[0].1.candidate_id, after_cand.id,
        "current binding, not the old snapshot's"
    );
    // Removed candidates never reappear through the cache.
    let cached = decide_cached(
        &[],
        &after_entities,
        &after_snap,
        "jev-1.13.0",
        &mat,
        &policy,
        &mut store,
    )
    .await
    .unwrap();
    assert!(cached.is_empty(), "no candidates means no reuse");
}
