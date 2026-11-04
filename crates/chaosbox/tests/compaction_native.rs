//! Native-boundary regressions on the real Rust custody and checkpoint path.
use chaosbox::compaction::{Capture, Journal};
use serde_json::{json, Value};

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

fn capture(records: Vec<Value>) -> Capture {
    Capture {
        version: 1,
        scope: "private:test".into(),
        repo: "test".into(),
        source: "opencode".into(),
        session: "ses_native".into(),
        records,
    }
}

fn user() -> Value {
    json!({"id":"msg_user","type":"user","text":"Preserve the exact requirements.","time":{"created":1}})
}

fn assistant() -> Value {
    json!({"id":"msg_assistant","type":"assistant","time":{"created":2,"completed":3},"content":[
        {"type":"tool","id":"call_test","name":"shell","state":{"status":"completed",
         "input":{"command":"test"},"content":[{"type":"text","text":"bounded preview"}],
         "metadata":{"truncated":true,"exitCode":0}}}
    ]})
}

fn settlement() -> Value {
    json!({"id":"msg_assistant:call_test","type":"tool-result","messageID":"msg_assistant",
        "callID":"call_test","tool":"shell","input":{"command":"test"},"status":"completed",
        "result":{"content":"Complete failure: repair remains pending.","metadata":{"exitCode":1}}})
}

#[test]
fn missing_or_malformed_native_content_never_authorizes_reduction() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    for malformed in [
        json!({"id":"msg_a","type":"assistant","time":{"completed":3}}),
        json!({"id":"msg_a","type":"assistant","time":{"completed":3},"content":"lost parts"}),
        json!({"id":"msg_a","type":"assistant","time":{"completed":3},"content":[{"type":"text"}]}),
        json!({"id":"msg_a","type":"assistant","time":{"completed":3},"content":[{"type":"unknown","text":"unvalidated"}]}),
    ] {
        assert!(journal.capture(&capture(vec![user(), malformed])).is_err());
    }
    assert_eq!(journal.pending().unwrap(), Vec::<String>::new());
}

#[test]
fn a_settlement_requires_its_actual_outcome_and_call_identity() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    for field in ["result", "input"] {
        let mut broken = settlement();
        broken.as_object_mut().unwrap().remove(field);
        assert!(journal.capture(&capture(vec![broken])).is_err());
    }
    let mut error = settlement();
    error["status"] = json!("error");
    assert!(journal.capture(&capture(vec![error])).is_err());
    let mut duplicate = assistant();
    let tool = duplicate["content"][0].clone();
    duplicate["content"].as_array_mut().unwrap().push(tool);
    assert!(journal.capture(&capture(vec![user(), duplicate])).is_err());
}

#[test]
fn a_truncated_early_result_is_custody_but_not_complete_output() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut early = settlement();
    early["result"]["metadata"]["truncated"] = json!(true);
    journal.capture(&capture(vec![early])).unwrap();
    let receipt = journal
        .capture(&capture(vec![user(), assistant()]))
        .unwrap();
    assert!(journal.plan(&receipt.id, 8000).is_err());
    assert_eq!(journal.pending().unwrap().len(), 2);
}

#[test]
fn malformed_truncation_markers_never_authorize_complete_tool_output() {
    for truncated in [
        json!("true"),
        json!("false"),
        json!(null),
        json!(0),
        json!({}),
    ] {
        let dir = private_dir();
        let mut journal = Journal::open(dir.path(), "private:test").unwrap();
        let mut early = settlement();
        early["result"]["metadata"]["truncated"] = truncated;
        journal.capture(&capture(vec![early])).unwrap();
        let receipt = journal
            .capture(&capture(vec![user(), assistant()]))
            .unwrap();
        assert!(journal.plan(&receipt.id, 8000).is_err());
        assert_eq!(journal.pending().unwrap().len(), 2);
    }
}

#[test]
fn string_output_settlements_preserve_exact_text_in_the_checkpoint() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut early = settlement();
    let text = "Actual output: \"quoted\"\nsecond line\\path";
    early["result"].as_object_mut().unwrap().remove("content");
    early["result"]["output"] = json!(text);
    early["result"]["metadata"]["truncated"] = json!(false);
    journal.capture(&capture(vec![early])).unwrap();
    let receipt = journal
        .capture(&capture(vec![user(), assistant()]))
        .unwrap();
    let plan = journal.plan(&receipt.id, 8000).unwrap();
    let projected: Value = plan
        .summary
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["id"] == "msg_assistant")
        .unwrap();
    assert_eq!(projected["content"][0]["state"]["content"][0]["text"], text);
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}

#[test]
fn malformed_embedded_tool_truncation_never_enters_custody() {
    for status in ["completed", "error"] {
        for truncated in [
            json!("true"),
            json!("false"),
            json!(null),
            json!(0),
            json!({}),
            json!([]),
        ] {
            let dir = private_dir();
            let mut journal = Journal::open(dir.path(), "private:test").unwrap();
            let mut record = assistant();
            let state = &mut record["content"][0]["state"];
            state["status"] = json!(status);
            if status == "error" {
                state["error"] = json!({"message":"shell failed"});
            }
            state["metadata"]["truncated"] = truncated;
            assert!(journal.capture(&capture(vec![user(), record])).is_err());
            assert!(journal.pending().unwrap().is_empty());
        }
    }
}

#[test]
fn complete_embedded_tool_output_can_be_planned_without_an_early_result() {
    for status in ["completed", "error"] {
        for truncated in [None, Some(false)] {
            let dir = private_dir();
            let mut journal = Journal::open(dir.path(), "private:test").unwrap();
            let mut record = assistant();
            let state = &mut record["content"][0]["state"];
            state["status"] = json!(status);
            state["content"] = json!([{"type":"text","text":"Complete embedded output"}]);
            if status == "error" {
                state["error"] = json!({"message":"shell failed"});
            }
            match truncated {
                Some(value) => state["metadata"]["truncated"] = json!(value),
                None => {
                    state["metadata"]
                        .as_object_mut()
                        .unwrap()
                        .remove("truncated");
                }
            }
            let receipt = journal.capture(&capture(vec![user(), record])).unwrap();
            let plan = journal.plan(&receipt.id, 8000).unwrap();
            assert!(plan.summary.contains("Complete embedded output"));
            assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
        }
    }
}

#[test]
fn the_checkpoint_uses_the_complete_outcome_instead_of_a_bounded_preview() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    journal.capture(&capture(vec![settlement()])).unwrap();
    let receipt = journal
        .capture(&capture(vec![user(), assistant()]))
        .unwrap();
    let plan = journal.plan(&receipt.id, 8000).unwrap();
    assert!(plan
        .summary
        .contains("Complete failure: repair remains pending."));
    assert!(plan.summary.contains("\"exitCode\":1"));
    assert!(!plan.summary.contains("bounded preview"));
    assert!(!plan.summary.contains("\"exitCode\":0"));
}

fn runtime_capture(root: &std::path::Path, directory: &str, name: &str, body: &str) -> String {
    let directory = root.join(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join(name);
    std::fs::write(&file, body).unwrap();
    file.to_str().unwrap().to_owned()
}

#[test]
fn paged_tool_results_are_complete_without_bounded_metadata() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut early = settlement();
    early["tool"] = json!("read");
    early["result"]["content"] = json!("page one of a long file");
    early["result"]["metadata"] = json!({"truncated": true});
    journal.capture(&capture(vec![early])).unwrap();
    let mut record = assistant();
    record["content"][0]["name"] = json!("read");
    record["content"][0]["state"]["metadata"] = json!({"truncated": true});
    record["content"][0]["state"]["content"] =
        json!([{"type":"text","text":"page one of a long file"}]);
    let receipt = journal.capture(&capture(vec![user(), record])).unwrap();
    let plan = journal.plan(&receipt.id, 8000).unwrap();
    assert!(plan.summary.contains("page one of a long file"));
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}

#[test]
fn file_backed_tool_output_is_archived_before_reduction() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let complete = "Complete repair output.\n".repeat(300);
    let path = runtime_capture(dir.path(), "shell", "sh_0123456789ab.out", &complete);
    let mut early = settlement();
    early["result"]["content"] = json!("bounded preview");
    early["result"]["metadata"] = json!({"truncated": true, "outputPath": path});
    journal.capture(&capture(vec![early])).unwrap();
    let receipt = journal
        .capture(&capture(vec![user(), assistant()]))
        .unwrap();
    let plan = journal.plan(&receipt.id, 120_000).unwrap();
    assert!(!plan.summary.contains("bounded preview"));
    assert_eq!(plan.externalized.len(), 1);
    let archived = journal.evidence(&plan.externalized[0]).unwrap();
    assert_eq!(archived["result"]["content"][0]["text"], json!(complete));
    assert_eq!(archived["result"]["metadata"]["truncated"], json!(false));
    assert!(archived["result"]["metadata"].get("outputPath").is_none());
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}

#[test]
fn bounded_previews_outside_runtime_captures_are_refused() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let complete = "unbounded text";
    let outside = dir.path().join("sh_0123456789ac.out");
    std::fs::write(&outside, complete).unwrap();
    let linked = runtime_capture(dir.path(), "shell", "sh_0123456789ad.out", complete);
    let link = dir.path().join("shell/sh_0123456789ae.out");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&linked, &link).unwrap();
    #[cfg(not(unix))]
    let _ = &linked;
    let oversized = runtime_capture(dir.path(), "tool-output", "tool_0123456789af", complete);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&oversized)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    #[cfg(unix)]
    let paths = [
        outside.to_str().unwrap(),
        link.to_str().unwrap(),
        oversized.as_str(),
    ];
    #[cfg(not(unix))]
    let paths = [outside.to_str().unwrap(), oversized.as_str()];
    for path in paths {
        let mut early = settlement();
        early["result"]["content"] = json!("bounded preview");
        early["result"]["metadata"] = json!({"truncated": true, "outputPath": path});
        journal.capture(&capture(vec![early])).unwrap();
        let receipt = journal
            .capture(&capture(vec![user(), assistant()]))
            .unwrap();
        assert!(journal.plan(&receipt.id, 8000).is_err(), "{path}");
    }
}

#[test]
fn bounded_shell_records_reference_the_complete_capture() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let complete = "full build log line\n".repeat(400);
    let path = runtime_capture(dir.path(), "shell", "sh_0123456789b0.out", &complete);
    let shell = json!({"id":"msg_shell","type":"shell","status":"exited","command":"build",
        "outputPath":path,
        "output":{"output":"[showing lines 901-1000 of 1000; full output saved to the capture]","truncated":true}});
    let receipt = journal.capture(&capture(vec![user(), shell])).unwrap();
    let plan = journal.plan(&receipt.id, 120_000).unwrap();
    assert!(!plan.summary.contains("showing lines"));
    assert_eq!(plan.externalized.len(), 1);
    let archived = journal.evidence(&plan.externalized[0]).unwrap();
    assert_eq!(archived["output"]["output"], json!(complete));
    assert_eq!(archived["output"]["truncated"], json!(false));
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}

#[test]
fn archived_shell_bodies_survive_runtime_retention() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let complete = "durable shell body\n".repeat(200);
    let path = runtime_capture(dir.path(), "shell", "sh_0123456789b2.out", &complete);
    let shell = json!({"id":"msg_shell","type":"shell","status":"exited","command":"build",
        "outputPath":path,
        "output":{"output":"bounded preview","truncated":true}});
    let first = journal
        .capture(&capture(vec![user(), shell.clone()]))
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let second = journal.capture(&capture(vec![user(), shell])).unwrap();
    assert_eq!(first.id, second.id);
    let plan = journal.plan(&second.id, 120_000).unwrap();
    assert_eq!(plan.externalized.len(), 1);
    assert_eq!(
        journal.evidence(&plan.externalized[0]).unwrap()["output"]["output"],
        json!(complete)
    );
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}

#[test]
fn legacy_bounded_settlements_are_repaired_from_the_runtime_capture() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let complete = "Complete legacy repair.\n".repeat(300);
    let path = runtime_capture(dir.path(), "shell", "sh_0123456789b1.out", &complete);
    let mut legacy = settlement();
    legacy["result"]["content"] = json!(format!(
        "bounded preview\n[showing lines 901-1000 of 1000; full output saved to {path}]"
    ));
    legacy["result"]["metadata"] = json!({"truncated": true, "exitCode": 1});
    let body = serde_json::to_string(&legacy).unwrap();
    let hash = chaosbox_core::sha256_hex(&[&body]);
    let receipt = journal
        .capture(&capture(vec![user(), assistant()]))
        .unwrap();
    let db = rusqlite::Connection::open(dir.path().join("memory.sqlite")).unwrap();
    db.execute(
        "INSERT INTO blobs VALUES (?1,?2)",
        rusqlite::params![hash, body],
    )
    .unwrap();
    db.execute(
        "INSERT INTO tool_sources VALUES ('opencode','ses_native','test','msg_assistant','call_test',?1)",
        [&hash],
    )
    .unwrap();
    drop(db);
    let plan = journal.plan(&receipt.id, 120_000).unwrap();
    assert!(!plan.summary.contains("bounded preview"));
    assert!(plan.summary.contains("\"exitCode\":1"));
    assert_eq!(plan.externalized.len(), 1);
    let archived = journal.evidence(&plan.externalized[0]).unwrap();
    assert_eq!(archived["result"]["content"][0]["text"], json!(complete));
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}

#[test]
fn rust_observation_selects_settled_records_and_reports_unsettled_work() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut running = assistant();
    running["content"][0]["state"]["status"] = json!("running");
    let source = capture(vec![user(), running.clone()]);
    assert!(journal.capture(&source).is_err());
    let observation = journal.observe(&source).unwrap();
    assert_eq!(observation.unsettled, vec!["msg_assistant"]);
    assert_eq!(observation.custody.unwrap().records, 1);
    let empty = journal.observe(&capture(vec![running])).unwrap();
    assert!(empty.custody.is_none());
    assert_eq!(journal.pending().unwrap().len(), 1);
}

#[test]
fn rust_budget_refusal_keeps_custody_and_never_persists_a_plan() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut source = user();
    source["text"] = json!("λ".repeat(1000));
    let receipt = journal.capture(&capture(vec![source])).unwrap();
    assert!(journal.plan_with_budget(&receipt.id, 8000, 2000).is_err());
    let db = rusqlite::Connection::open(dir.path().join("memory.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM plans", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    drop(db);
    let plan = journal.plan_with_budget(&receipt.id, 8000, 8000).unwrap();
    drop(journal);
    let journal = Journal::open(dir.path(), "private:test").unwrap();
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
    assert_eq!(journal.pending().unwrap(), vec![receipt.id]);
}

#[test]
fn plan_verification_binds_dispositions_and_rechecks_complete_custody() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let receipt = journal.capture(&capture(vec![user()])).unwrap();
    let plan = journal.plan(&receipt.id, 8000).unwrap();
    drop(journal);
    let db = rusqlite::Connection::open(dir.path().join("memory.sqlite")).unwrap();
    let mut tampered = serde_json::to_value(&plan).unwrap();
    tampered["externalized"] = json!(["a".repeat(64)]);
    db.execute(
        "UPDATE plans SET body=?1 WHERE id=?2",
        rusqlite::params![tampered.to_string(), plan.id],
    )
    .unwrap();
    drop(db);
    let journal = Journal::open(dir.path(), "private:test").unwrap();
    assert!(journal.verify_plan(&plan.id).is_err());
    drop(journal);
    let db = rusqlite::Connection::open(dir.path().join("memory.sqlite")).unwrap();
    db.execute(
        "UPDATE plans SET body=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(&plan).unwrap(), plan.id],
    )
    .unwrap();
    db.execute("DELETE FROM blobs", []).unwrap();
    drop(db);
    let journal = Journal::open(dir.path(), "private:test").unwrap();
    assert!(journal.verify_plan(&plan.id).is_err());
}

#[test]
fn settled_shells_require_an_outcome_and_truncation_is_explicit() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut shell = json!({"id":"msg_shell","type":"shell","status":"exited","command":"test"});
    assert!(journal
        .capture(&capture(vec![user(), shell.clone()]))
        .is_err());
    shell["output"] = json!({"output":"bounded preview","truncated":"false"});
    assert!(journal
        .capture(&capture(vec![user(), shell.clone()]))
        .is_err());
    shell["output"]["truncated"] = json!(true);
    let receipt = journal
        .capture(&capture(vec![user(), shell.clone()]))
        .unwrap();
    assert!(journal.plan(&receipt.id, 8000).is_err());
    shell["output"]["truncated"] = json!(false);
    let receipt = journal.capture(&capture(vec![user(), shell])).unwrap();
    assert!(journal
        .plan(&receipt.id, 8000)
        .unwrap()
        .summary
        .contains("bounded preview"));
}

#[test]
fn legacy_prompt_attachments_need_inline_custody_and_recoverable_references() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:test").unwrap();
    let mut prompt = json!({"id":"msg_attachment","type":"user","prompt":{"parts":[
        {"type":"text","text":"Preserve this attachment."},
        {"type":"file","uri":"file:///ephemeral/input.png"}
    ]}});
    let receipt = journal.capture(&capture(vec![prompt.clone()])).unwrap();
    assert!(journal.plan(&receipt.id, 8000).is_err());
    let bytes = format!("data:image/png;base64,{}", "A".repeat(20_000));
    prompt["prompt"]["parts"][1]["uri"] = json!(bytes);
    let receipt = journal.capture(&capture(vec![prompt.clone()])).unwrap();
    let plan = journal.plan(&receipt.id, 8000).unwrap();
    assert!(plan.summary.contains("/prompt/parts/1/uri"));
    assert_eq!(journal.evidence(&plan.externalized[0]).unwrap(), prompt);
    assert_eq!(journal.verify_plan(&plan.id).unwrap(), plan);
}
