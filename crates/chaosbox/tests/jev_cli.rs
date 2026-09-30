//! Operator-only typed evaluation rejects invalid input before loading credentials.

use std::{fs, process::Command};

fn evaluate(value: &serde_json::Value, consent: bool) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("request.json");
    fs::write(&input, serde_json::to_vec(value).unwrap()).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chaosbox"));
    cmd.args(["jev", "evaluate", "--input"])
        .arg(input)
        .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
        .env_remove("TYPESAFE_API_KEY");
    if consent {
        cmd.arg("--privacy-reviewed");
    }
    cmd.output().unwrap()
}

fn request() -> serde_json::Value {
    serde_json::json!({"model":"jev-1.13.0", "state":"Synthetic example", "questions":{
        "kind":{"type":"choice", "instructions":"Classify the example",
        "criteria":{"yes":"Supported", "unknown":"Insufficient evidence"}}
    }})
}

#[test]
fn capabilities_report_the_enforced_policy_without_credentials() {
    let output = Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(["jev", "capabilities"])
        .env_remove("CHAOSBOX_JEV_API_KEY_FILE")
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"version":1, "model":chaosbox_jev::JEV_MODEL_PINNED,
        "endpoint":chaosbox_jev::JEV_ENDPOINT, "receipt_version":1,
        "strict_model_identity":true, "redirects":false})
    );
}

#[test]
fn consent_and_pinned_typed_input_are_required() {
    let output = evaluate(&request(), false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("privacy-reviewed"));
    for (key, value) in [
        ("model", serde_json::json!("jev-latest")),
        ("questions", serde_json::json!({})),
        ("state", serde_json::json!(null)),
    ] {
        let mut input = request();
        input[key] = value;
        let output = evaluate(&input, true);
        assert!(!output.status.success());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["sent_requests"], 0);
        assert_eq!(result["error"], "invalid_request");
        assert!(result["response"].is_null());
    }
}

#[test]
fn missing_credentials_produce_a_bounded_receipt_without_source_text() {
    let output = evaluate(&request(), true);
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["sent_requests"], 0);
    assert_eq!(result["error"], "auth");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Synthetic example"));
}
