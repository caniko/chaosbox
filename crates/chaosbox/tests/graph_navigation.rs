//! Build-bound navigation regression checks.
use chaosbox::navigation::{context, summary};
use serde_json::json;

fn graph() -> serde_json::Value {
    json!({"build_id":"build:reviewed","generation":2,"snapshots":["snap:one"],
        "nodes":[{"id":"a","label":"entry","source_file":"main.rs"},{"id":"b","label":"worker","source_file":"worker.rs"},{"id":"c","label":"isolated","source_file":"note.md"}],
        "links":[{"id":"ab","source":"a","target":"b","rel_type":"references"}]})
}

#[test]
fn navigation_keeps_isolated_nodes_and_build_bound_evidence() {
    let result = summary(&graph(), 10).unwrap();
    assert_eq!(result["nodes"], 3);
    assert_eq!(result["edges"], 1);
    assert_eq!(result["communities"].as_array().unwrap().len(), 2);
    assert_eq!(result["build_id"], "build:reviewed");
    let packet = context(&graph(), "entry", 2, 5, 4096).unwrap();
    assert_eq!(packet["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(packet["links"][0]["id"], "ab");
    assert_eq!(packet["links"][0]["rel_type"], "references");
    assert_eq!(packet["exhaustive"], false);
}

#[test]
fn navigation_rejects_dangling_edges_and_excessive_output() {
    let mut broken = graph();
    broken["links"][0]["target"] = json!("missing");
    assert!(summary(&broken, 10).is_err());
    assert!(context(&graph(), "entry", 100, 5, 4096).is_err());
    assert!(context(&graph(), "entry", 1, 5, 1).is_err());
}

#[test]
fn navigation_is_order_independent_and_reports_truncation() {
    let original = graph();
    let mut reordered = original.clone();
    reordered["nodes"].as_array_mut().unwrap().reverse();
    assert_eq!(
        summary(&original, 10).unwrap(),
        summary(&reordered, 10).unwrap()
    );
    let packet = context(&original, "entry", 2, 1, 4096).unwrap();
    assert_eq!(packet["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(packet["truncated"], true);
    assert!(packet["links"].as_array().unwrap().is_empty());
}
