//! Streaming export preserves the existing JSON contract without a second graph.

use chaosbox::{export_json, write_export_json, Materialization, Pipeline};
use chaosbox_core::{EffectivePolicy, GraphBuild};
use chaosbox_store::MemoryStore;

#[tokio::test]
async fn streamed_export_matches_existing_json_bytes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-repo");
    let policy = EffectivePolicy::new(&[], "local", "none").unwrap();
    let (snapshot, extraction, _) =
        Pipeline::<MemoryStore>::snapshot_extract("demo", &root, 40, &policy).unwrap();
    let mut pipeline = Pipeline::<MemoryStore>::new();
    let build = pipeline
        .build_and_publish(
            "demo",
            &snapshot,
            &extraction,
            &[],
            &Materialization::default(),
            None,
        )
        .await
        .unwrap();
    assert!(!build.nodes.is_empty());
    assert!(!build.edges.is_empty());
    for graph in [build, GraphBuild::new("empty", vec![], 1)] {
        let mut bytes = Vec::new();
        write_export_json(&graph, &mut bytes).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            serde_json::to_string(&export_json(&graph)).unwrap()
        );
    }
}

#[test]
fn streamed_export_reports_writer_failure() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(write_export_json(&GraphBuild::new("r", vec![], 1), Broken).is_err());
}
