//! PostgreSQL catalog provenance and deterministic publication regressions.
use chaosbox::postgres::{Capture, Catalog, publish};
use chaosbox_store::{MemoryStore, Store};

fn catalog() -> Catalog {
    serde_json::from_value(serde_json::json!({"database":"canix","schema":"public","role":"chaosbox-postgresql",
        "server_version":"180006","read_only":"on","schema_exists":true,"schema_usage":true,
        "omissions":{"row_data":"not collected"},"records":[
            {"key":"relation:1","kind":"table","schema":"public","name":"parent","parent":null,"target":null,"definition":null,"details":{}},
            {"key":"relation:2","kind":"table","schema":"public","name":"child","parent":null,"target":null,"definition":null,"details":{}},
            {"key":"constraint:3","kind":"constraint","schema":"public","name":"child_parent_fk","parent":"relation:2","target":"relation:1","definition":"FOREIGN KEY (parent_id) REFERENCES parent(id)","details":{"type":"f","columns":[2],"target_columns":[1]}}
        ]})).unwrap()
}

#[tokio::test]
async fn readonly_catalog_retains_foreign_keys_and_quoted_receipts() {
    let capture = Capture::new(catalog(), 1000).unwrap();
    let mut store = MemoryStore::new();
    let build = publish(&mut store, &capture, "atlas-postgresql", 1, None)
        .await
        .unwrap();
    assert_eq!(store.stats().decisions, 0);
    let coverage = build.coverage.as_ref().unwrap();
    assert_eq!(coverage.structural_relations, 0);
    assert_eq!(coverage.decision_relations, 0);
    assert_eq!(
        coverage.catalog.as_ref().unwrap().relations,
        build.edges.len()
    );
    assert_eq!(
        build
            .edges
            .values()
            .filter(|r| r.rel_type == chaosbox_core::RelationType::References)
            .count(),
        1
    );
    let evidence = store.export_staged().evidence;
    assert!(
        evidence.iter().any(|e| e.text.contains("FOREIGN KEY")
            && e.producer.as_deref() == Some("postgres-catalog-v1"))
    );
    assert!(
        evidence
            .iter()
            .all(|e| e.span.is_some() && e.snapshot == capture.snapshot())
    );
    assert!(store.active("atlas-postgresql").is_some());
}

#[test]
fn catalog_is_order_independent_and_rejects_tampering_or_wrong_scope() {
    let original = catalog();
    let capture = Capture::new(original.clone(), 1000).unwrap();
    let mut reordered = original;
    reordered.records.reverse();
    assert_eq!(capture, Capture::new(reordered, 1000).unwrap());
    let mut changed = capture.clone();
    changed.catalog.records[0].name.push_str("tampered");
    assert!(changed.validate().is_err());
    let mut wrong = catalog();
    wrong.records[0].schema = "private".into();
    assert!(Capture::new(wrong, 1000).is_err());
}

#[test]
fn unavailable_schema_and_duplicate_catalog_ids_fail_closed() {
    let mut missing = catalog();
    missing.schema_exists = false;
    assert!(Capture::new(missing, 1000).is_err());
    let mut duplicate = catalog();
    duplicate.records.push(duplicate.records[0].clone());
    assert!(Capture::new(duplicate, 1000).is_err());
}
