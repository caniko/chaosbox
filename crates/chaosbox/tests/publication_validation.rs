//! Invalid structural batches must fail before persisting any observations.

use chaosbox::{Materialization, Pipeline};
use chaosbox_core::EffectivePolicy;
use chaosbox_store::MemoryStore;

#[tokio::test]
async fn duplicate_fact_does_not_partially_stage_evidence_or_claims() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("lib.rs"), "pub fn one() {}\n").unwrap();
    let policy = EffectivePolicy::new(&[], "local", "none").unwrap();
    let (snapshot, mut extraction, _) =
        Pipeline::<MemoryStore>::snapshot_extract("r", root.path(), 0, &policy).unwrap();
    extraction.facts.push(extraction.facts[0].clone());
    let mut pipeline = Pipeline::<MemoryStore>::new();
    let before = pipeline.store.stats();
    let error = pipeline
        .build_and_publish(
            "r",
            &snapshot,
            &extraction,
            &[],
            &Materialization::default(),
            None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("duplicate"), "{error}");
    assert_eq!(pipeline.store.stats(), before);
}
