//! Budget preflight: `uncached_decisions` counts exactly the requests
//! `decide` would spend, before any spend happens — cache hits are free,
//! recorded failures always re-ask.

use std::{collections::BTreeMap, path::PathBuf};

use chaosbox::{uncached_decisions, FixtureResponder, Materialization, Pipeline};
use chaosbox_core::DecisionOutcome;
use chaosbox_store::{MemoryStore, Store as _};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-repo")
}

#[tokio::test]
async fn preflight_counts_uncached_then_cached_then_failed() {
    let root = fixture_root();
    let (snap, ext, cat) = Pipeline::<MemoryStore>::snapshot_extract("demo", &root, 200).unwrap();
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
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
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
        &mut responder,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &mut store,
    )
    .await
    .unwrap();
    assert_eq!(decided.len(), cands.len());
    let pending = uncached_decisions(
        cands,
        &entities,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 0, "every decision cached: nothing to spend");

    // A recorded Failed outcome always re-asks, under an unchanged key.
    // Seed a fresh store with the full decided set, one of them Failed:
    // exactly that one candidate must come back pending.
    let mut failed = decided[0].1.clone();
    failed.outcome = DecisionOutcome::Failed("preflight test".into());
    let mut retry_store = MemoryStore::default();
    retry_store.put_decision(failed).await.unwrap();
    for (_, d, _) in decided.iter().skip(1) {
        retry_store.put_decision(d.clone()).await.unwrap();
    }
    let pending = uncached_decisions(
        cands,
        &entities,
        chaosbox_jev::JEV_MODEL_PINNED,
        &mat,
        &retry_store,
    )
    .await
    .unwrap();
    assert_eq!(pending, 1, "recorded failure must re-ask, once");
}
