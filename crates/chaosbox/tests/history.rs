//! Interrupted-history projection preserves evidence without replaying work.
use chaosbox::history::recover_draft;
use serde_json::json;

#[test]
fn preserves_role_text_completed_results_and_exact_original() {
    let source = json!({"id":"msg_draft","type":"assistant","agent":"code","model":{"providerID":"test","id":"test"},"time":{"created":100},"retry":{"at":999},"content":[
        {"type":"text","text":"Partial finding: observed output follows."},
        {"type":"tool","id":"done","name":"shell","state":{"status":"completed","input":{"command":"echo done"},"content":[{"type":"text","text":"done"}]}},
        {"type":"tool","id":"running","name":"shell","state":{"status":"running","input":{"command":"deploy"},"metadata":{"partial":"still waiting"}}},
        {"type":"tool","id":"streaming","name":"shell","state":{"status":"streaming","input":"{\"command\":"}}
    ]});
    let result = recover_draft(source.clone(), &"a".repeat(64), 200).unwrap();
    assert!(result.changed);
    assert_eq!(result.original, source);
    assert_eq!(result.message["type"], "assistant");
    assert_eq!(result.message["content"][0], source["content"][0]);
    assert_eq!(result.message["content"][1], source["content"][1]);
    assert_eq!(result.message["content"][2]["state"]["status"], "error");
    assert_eq!(
        result.message["content"][3]["state"]["input"]["_archivedPartialInput"],
        "{\"command\":"
    );
    assert!(result.message.get("retry").is_none());
    assert_eq!(result.message["finish"], "error");
    assert_eq!(
        result.message["metadata"]["chaosboxMigrationDraft"]["originalCompletionKnown"],
        false
    );
    assert_eq!(
        result.message["metadata"]["chaosboxMigrationDraft"]["originalTime"],
        source["time"]
    );
    let extracted = chaosbox::intelligence::extract(
        &result.message.to_string(),
        "opencode",
        "s",
        "private:test",
        &["test".into()],
        10,
    )
    .unwrap();
    assert!(extracted.candidates.is_empty());
    assert_eq!(extracted.excluded_derived, 1);
    let again = recover_draft(result.message.clone(), &"a".repeat(64), 300).unwrap();
    assert!(!again.changed);
    assert_eq!(again.message, result.message);
}

#[test]
fn never_guesses_unknown_operations_or_settlement_times() {
    assert!(recover_draft(
        json!({"id":"msg_x","type":"shell","status":"running"}),
        &"a".repeat(64),
        100
    )
    .is_err());
    assert!(recover_draft(
        json!({"id":"msg_x","type":"assistant","time":{"created":200},"content":[]}),
        &"a".repeat(64),
        100
    )
    .is_err());
    assert!(recover_draft(json!({"id":"msg_x","type":"assistant","time":{"created":0},"content":[{"type":"tool","state":{"status":"mystery"}}]}),&"a".repeat(64),100).is_err());
}
