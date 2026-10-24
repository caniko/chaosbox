//! Compiler evidence reaches publication and readback without model decisions.

use chaosbox::{GraphReader, Materialization, Pipeline};
use chaosbox_core::RelationType;
use chaosbox_extract::{
    compiler::{AnalysisInputs, AnalysisSettings, Receipt},
    Snapshot,
};
use chaosbox_store::{MemoryReader, MemoryStore, Store};
use protobuf::Message;

fn fixture() -> (Snapshot, chaosbox_extract::Extraction) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/indexer-smoke/rust")
        .canonicalize()
        .unwrap();
    let bytes = std::fs::read(root.parent().unwrap().join("rust-default.scip")).unwrap();
    let index = scip::types::Index::parse_from_bytes(&bytes).unwrap();
    let mut inputs = AnalysisInputs::capture("compiler", &root, &[], &[]).unwrap();
    inputs
        .root
        .clone_from(&index.metadata.as_ref().unwrap().project_root);
    let settings = AnalysisSettings {
        command: vec![
            "rust-analyzer".into(),
            "scip".into(),
            "--output".into(),
            "{index}".into(),
        ],
        declared: ["toolchain", "configuration", "target", "features"]
            .into_iter()
            .map(|k| (k.into(), "archived fixture".into()))
            .collect(),
        unspecified_encoding: None,
    };
    let receipt = Receipt::seal(inputs, settings, &bytes).unwrap();
    let snapshot = Snapshot::capture("compiler", &root).unwrap();
    let mut extraction = chaosbox_extract::extract_snapshot(&snapshot);
    chaosbox_extract::compiler::normalize(&snapshot, &receipt, &bytes)
        .unwrap()
        .attach(&mut extraction)
        .unwrap();
    (snapshot, extraction)
}

#[tokio::test]
async fn compiler_facts_publish_with_zero_budget_and_retain_anchors_and_citations() {
    let (snapshot, extraction) = fixture();
    assert!(chaosbox_extract::build_candidates(&extraction, 0)
        .candidates
        .is_empty());
    let mut pipe = Pipeline::<MemoryStore>::new();
    let build = pipe
        .build_and_publish(
            "compiler",
            &snapshot,
            &extraction,
            &[],
            &Materialization::default(),
            None,
        )
        .await
        .unwrap();
    let reader = GraphReader::pinned(MemoryReader::from_store(&pipe.store), "compiler")
        .await
        .unwrap();
    assert_eq!(
        reader.coverage.as_ref().unwrap().compiler,
        extraction.compiler
    );
    let exported = reader.export().await.unwrap();
    assert!(exported["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["compiler"]["anchor"].is_string()));
    for edge in build
        .edges
        .values()
        .filter(|e| e.rel_type == RelationType::References)
    {
        let source = &build.nodes[&edge.from];
        let target = &build.nodes[&edge.to];
        assert_eq!(
            source.compiler.as_ref().unwrap().anchor,
            target.compiler.as_ref().unwrap().anchor
        );
        let evidence = reader.evidence(&edge.id).await.unwrap();
        assert_eq!(evidence["evidence"][0]["text"], source.name);
        assert_eq!(
            evidence["evidence"][0]["citation"]["span"],
            serde_json::to_value(&source.span).unwrap()
        );
        assert_eq!(
            reader.lookup(&edge.from).await.unwrap().unwrap().compiler,
            source.compiler
        );
    }
    assert!(pipe.store.export_staged().decisions.is_empty());
    let mut tampered = extraction.clone();
    tampered.compiler.as_mut().unwrap().references += 1;
    assert!(pipe
        .build_and_publish(
            "compiler",
            &snapshot,
            &tampered,
            &[],
            &Materialization::default(),
            Some(build.id.clone())
        )
        .await
        .is_err());
    assert_eq!(pipe.store.active("compiler").unwrap().id, build.id);
}

#[tokio::test]
#[ignore = "requires disposable TypeDB via scripts/test-typedb.sh"]
async fn compiler_facts_survive_fresh_typedb_readback() {
    use chaosbox_typedb::{
        reader::TypeDbReader,
        store::{TypeDbConfig, TypeDbStore},
    };
    let (snapshot, extraction) = fixture();
    let config = TypeDbConfig {
        address: std::env::var("CHAOSBOX_TYPEDB_ADDR").unwrap(),
        username: "admin".into(),
        password: std::fs::read_to_string(std::env::var("CHAOSBOX_TYPEDB_PASSWORD_FILE").unwrap())
            .unwrap()
            .trim()
            .into(),
        database: format!("compiler_{}", std::process::id()),
    };
    let mut store = TypeDbStore::new(config.clone());
    store.migrate().await.unwrap();
    let mut pipe = Pipeline {
        store,
        generation: 0,
    };
    let build = pipe
        .build_and_publish(
            "compiler",
            &snapshot,
            &extraction,
            &[],
            &Materialization::default(),
            None,
        )
        .await
        .unwrap();
    let mut backend = TypeDbReader::new(config);
    backend.connect().await.unwrap();
    let reader = GraphReader::pinned(backend, "compiler").await.unwrap();
    assert_eq!(
        reader.coverage.as_ref().unwrap().compiler,
        extraction.compiler
    );
    let edge = build
        .edges
        .values()
        .find(|e| e.rel_type == RelationType::References)
        .unwrap();
    assert_eq!(
        reader.lookup(&edge.from).await.unwrap().unwrap().compiler,
        build.nodes[&edge.from].compiler
    );
    let evidence = reader.evidence(&edge.id).await.unwrap();
    assert_eq!(
        evidence["evidence"][0]["citation"]["span"],
        serde_json::to_value(&build.nodes[&edge.from].span).unwrap()
    );
    assert!(evidence["evidence"][0]["producer"]
        .as_str()
        .unwrap()
        .starts_with("scip-receipt-v1/"));
}
