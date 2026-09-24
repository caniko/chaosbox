//! Budget preflight: `uncached_decisions` counts exactly the requests
//! `decide` would spend, before any spend happens — cache hits are free,
//! recorded failures always re-ask.

use std::{collections::BTreeMap, path::PathBuf};

use chaosbox::{uncached_decisions, FixtureResponder, Materialization, Pipeline};
use chaosbox_store::{MemoryStore, Store as _};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-repo")
}

fn test_policy() -> chaosbox_core::EffectivePolicy {
    chaosbox_core::EffectivePolicy::new(&[], "local", "typesafe-jev").unwrap()
}

#[tokio::test]
async fn preflight_counts_uncached_then_cached_then_failed() {
    let root = fixture_root();
    let policy = test_policy();
    let (snap, ext, cat) =
        Pipeline::<MemoryStore>::snapshot_extract("demo", &root, 200, &policy).unwrap();
    let cands = &cat.candidates;
    assert!(!cands.is_empty(), "fixture must yield candidates");
    let entities: BTreeMap<_, _> = ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let mat = Materialization::default();

    // Fresh store: every candidate would cost one request.
    let mut store = MemoryStore::default();
    store
        .ensure_snapshot_files(&snap.id, "demo", &snap.snapshot_files())
        .await
        .unwrap();
    let pending = uncached_decisions(
        cands,
        &entities,
        &snap,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &policy,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending, cands.len(), "fresh store: all uncached");

    // After decide, every decision reuses the cache: zero pending.
    let mut responder = FixtureResponder::new(true);
    let decided = Pipeline::<MemoryStore>::decide(
        cands,
        &entities,
        &snap,
        &mut responder,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &policy,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(decided.len(), cands.len());
    let pending = uncached_decisions(
        cands,
        &entities,
        &snap,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &policy,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 0, "every decision cached: nothing to spend");

    // A recorded Failed outcome always re-asks, under an unchanged key.
// Seed a fresh store with the reusable inferences for all but one
    // candidate (Failed stores no inference by construction): exactly the
    // missing one must come back pending.
    let mut retry_store = MemoryStore::default();
    for (_, d, _) in decided.iter().skip(1) {
        let raw = d.raw_answer.clone().expect("fresh inference has raw");
        retry_store
            .put_inference(chaosbox_core::InferenceRecord {
                reuse_key: d.reuse_key.clone(),
                raw,
                model_requested: d.model_requested.clone(),
                model_returned: d.model_returned.clone(),
            })
            .await
            .unwrap();
        retry_store.put_decision(d.clone()).await.unwrap();
    }
    let pending = uncached_decisions(
        cands,
        &entities,
        &snap,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &policy,
        &retry_store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 1, "recorded failure must re-ask, once");
}
