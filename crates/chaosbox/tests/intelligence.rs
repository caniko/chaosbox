//! Selectivity and provenance gates; deterministic protocol fixtures only.
use chaosbox::intelligence::{assess, extract, mcp_query, questions, Bundle, Outcome};
use chaosbox_core::intelligence::{IntelligenceCandidate, IntelligenceStatus};
use chaosbox_jev::{
    Answer, ChoiceAnswer, NoulAnswer, Question, SystemOneResponse, Usage, JEV_MODEL_PINNED,
};

fn candidate(message: &str, text: &str, speaker: &str) -> IntelligenceCandidate {
    let input = if speaker == "user" {
        serde_json::json!({"id":message,"type":"user","text":text,"time":{"created":if message=="m2" {2000} else {1000}}})
    } else {
        serde_json::json!({"id":message,"type":"assistant","content":[{"type":"text","text":text}]})
    };
    extract(
        &input.to_string(),
        "opencode",
        "session-one",
        "private:can",
        &["canix".into()],
        20,
    )
    .unwrap()
    .candidates
    .remove(0)
}

fn answer(candidate: &IntelligenceCandidate, bundle: &Bundle, novelty: &str) -> SystemOneResponse {
    let (_, asked, _) = questions(candidate, bundle).unwrap();
    let answers = asked
        .into_iter()
        .map(|(name, question)| {
            let value = match question {
                Question::Noul { .. } => Answer::Noul(NoulAnswer { noul: 0.99 }),
                Question::Choice { criteria, .. } => {
                    let choice = match name.as_str() {
                        "kind" => "constraint",
                        "utility" => "reusable",
                        "novelty" => novelty,
                        _ => panic!("unexpected question"),
                    };
                    Answer::Choice(ChoiceAnswer {
                        choice: choice.into(),
                        confidence: 0.99,
                        probabilities: criteria
                            .keys()
                            .map(|k| (k.clone(), f64::from(k == choice)))
                            .collect(),
                    })
                }
                Question::Score { .. } => panic!("unexpected question type"),
            };
            (name, value)
        })
        .collect();
    SystemOneResponse {
        model: JEV_MODEL_PINNED.into(),
        answers,
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
        },
    }
}

#[test]
fn extraction_keeps_verbatim_lineage_and_excludes_summary_echoes() {
    let input = [
        serde_json::json!({"id":"u1","type":"user","text":"We must preserve the direnv approval boundary."}),
        serde_json::json!({"id":"c1","type":"compaction","summary":"We must preserve the direnv approval boundary."}),
        serde_json::json!({"id":"s1","type":"synthetic","text":"We must preserve the direnv approval boundary."}),
    ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
    let result = extract(
        &input,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        10,
    )
    .unwrap();
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.excluded_derived, 2);
    assert_eq!(result.candidates[0].evidence.pointer, "/text");
    assert_eq!(
        result.candidates[0].evidence.quote,
        "We must preserve the direnv approval boundary."
    );
    let repeated = extract(
        &input,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        10,
    )
    .unwrap();
    assert_eq!(result.candidates, repeated.candidates);
    let other = extract(
        &input,
        "opencode",
        "other-session",
        "private:can",
        &["canix".into()],
        10,
    )
    .unwrap();
    assert_ne!(result.candidates[0].id, other.candidates[0].id);
}

#[test]
fn high_utility_cannot_compensate_for_unsupported_claims() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    for gate in ["support", "atomic", "scope", "durable"] {
        let mut bundle = Bundle::new("private:can");
        let mut response = answer(&c, &bundle, "novel");
        response
            .answers
            .insert(gate.into(), Answer::Noul(NoulAnswer { noul: 0.4 }));
        assert_eq!(
            assess(&c, &mut bundle, response).unwrap().outcome,
            Outcome::Abstained
        );
        assert!(bundle.records.is_empty());
        assert_eq!(bundle.assessments.len(), 1);
    }
}

#[test]
fn clear_negative_evidence_is_rejected_but_near_admission_is_abstained() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    for (support, expected) in [(0.01, Outcome::Rejected), (0.94, Outcome::Abstained)] {
        let mut bundle = Bundle::new("private:can");
        let mut response = answer(&c, &bundle, "novel");
        response
            .answers
            .insert("support".into(), Answer::Noul(NoulAnswer { noul: support }));
        assert_eq!(assess(&c, &mut bundle, response).unwrap().outcome, expected);
        assert!(bundle.records.is_empty());
        assert_eq!(bundle.assessments.len(), 1);
    }
}

#[test]
fn assessment_cache_changes_for_new_neighbors_but_not_its_own_materialization() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let (_, _, before) = questions(&c, &bundle).unwrap();
    let response = answer(&c, &bundle, "novel");
    assess(&c, &mut bundle, response).unwrap();
    assert_eq!(questions(&c, &bundle).unwrap().2, before);
    let other = candidate(
        "m2",
        "Direnv approval must remain explicit for protected projects.",
        "user",
    );
    let response = answer(&other, &bundle, "novel");
    assess(&other, &mut bundle, response).unwrap();
    assert_ne!(questions(&c, &bundle).unwrap().2, before);
}

#[test]
fn missing_fields_substituted_models_and_bad_distribution_fail_without_mutation() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let before = serde_json::to_value(&bundle).unwrap();
    let mut missing = answer(&c, &bundle, "novel");
    missing.answers.remove("scope");
    assert!(assess(&c, &mut bundle, missing).is_err());
    let mut wrong = answer(&c, &bundle, "novel");
    wrong.model = "fixture-or-other-model".into();
    assert!(assess(&c, &mut bundle, wrong).is_err());
    let mut incomplete = answer(&c, &bundle, "novel");
    if let Answer::Choice(choice) = incomplete.answers.get_mut("kind").unwrap() {
        choice.probabilities.remove("finding");
    }
    assert!(assess(&c, &mut bundle, incomplete).is_err());
    assert_eq!(before, serde_json::to_value(&bundle).unwrap());
}

#[test]
fn duplicates_merge_occurrences_without_becoming_new_knowledge() {
    let first = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let second = candidate(
        "m2",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let response = answer(&first, &bundle, "novel");
    assess(&first, &mut bundle, response).unwrap();
    let id = bundle.records[0].id.clone();
    let response = answer(&second, &bundle, &format!("duplicate:{id}"));
    assess(&second, &mut bundle, response).unwrap();
    assert_eq!(bundle.records.len(), 1);
    assert_eq!(bundle.records[0].evidence.len(), 2);
    assert_eq!(bundle.records[0].statement, first.evidence.quote);
    bundle.validate().unwrap();
}

#[test]
fn contradiction_preserves_both_sides_and_explicit_supersession_is_attributed() {
    let first = candidate(
        "m1",
        "We must require manual direnv approval for this project.",
        "user",
    );
    let second = candidate(
        "m2",
        "We must require automatic direnv approval for this project.",
        "user",
    );
    for operation in ["contradicts", "supersedes"] {
        let mut bundle = Bundle::new("private:can");
        let response = answer(&first, &bundle, "novel");
        assess(&first, &mut bundle, response).unwrap();
        let id = bundle.records[0].id.clone();
        let response = answer(&second, &bundle, &format!("{operation}:{id}"));
        assess(&second, &mut bundle, response).unwrap();
        let previous = bundle.records.iter().find(|r| r.id == id).unwrap();
        assert_eq!(
            previous.status,
            if operation == "contradicts" {
                IntelligenceStatus::Disputed
            } else {
                IntelligenceStatus::Superseded
            }
        );
        assert_eq!(
            bundle
                .context("private:can", "canix", "direnv", 5, 12_000)
                .unwrap()
                .len(),
            if operation == "contradicts" { 2 } else { 1 }
        );
        bundle.validate().unwrap();
    }
}

#[test]
fn assistant_reports_cannot_supersede_user_policy() {
    let first = candidate(
        "m1",
        "We must require manual direnv approval for this project.",
        "user",
    );
    let second = candidate(
        "m2",
        "We must require automatic direnv approval for this project.",
        "assistant",
    );
    let mut bundle = Bundle::new("private:can");
    let response = answer(&first, &bundle, "novel");
    assess(&first, &mut bundle, response).unwrap();
    let response = answer(
        &second,
        &bundle,
        &format!("supersedes:{}", bundle.records[0].id),
    );
    assert_eq!(
        assess(&second, &mut bundle, response).unwrap().outcome,
        Outcome::Abstained
    );
    assert_eq!(bundle.records.len(), 1);
    assert_eq!(bundle.records[0].status, IntelligenceStatus::Admitted);
}

#[test]
fn retrieval_is_scoped_bounded_and_read_only() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let response = answer(&c, &bundle, "novel");
    assess(&c, &mut bundle, response).unwrap();
    assert!(bundle
        .context("private:other", "canix", "direnv", 5, 12_000)
        .is_err());
    assert!(bundle
        .context("private:can", "unrelated", "direnv", 5, 12_000)
        .unwrap()
        .is_empty());
    assert!(bundle
        .context("private:can", "canix", "direnv", 5, 256)
        .unwrap()
        .is_empty());
    let response = mcp_query(
        &bundle,
        "intelligence_context",
        &serde_json::json!({"repo":"canix","query":"direnv"}),
    )
    .unwrap();
    assert_eq!(response["records"].as_array().unwrap().len(), 1);
    assert!(mcp_query(&bundle, "intelligence_assess", &serde_json::json!({})).is_err());
    assert!(mcp_query(
        &bundle,
        "intelligence_context",
        &serde_json::json!({"repo":"canix","query":"direnv","bundle":"/other-scope"})
    )
    .is_err());
    let id = bundle.records[0].id.clone();
    assert!(mcp_query(
        &bundle,
        "intelligence_evidence",
        &serde_json::json!({"repo":"other","id":id})
    )
    .unwrap()
    .is_null());
    bundle.records[0].statement = "Invented replacement statement".into();
    bundle.records[0].evidence[0].quote = "Invented replacement statement".into();
    assert!(bundle.validate().is_err());
}

#[test]
fn extraction_reports_coverage_instead_of_silent_truncation() {
    let input = serde_json::json!({"id":"u1","type":"user","text":"We must preserve native process permissions.\nWe must preserve environment removals at launch."}).to_string();
    let result = extract(&input, "opencode", "s", "private:can", &["canix".into()], 1).unwrap();
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.omitted, 1);
    assert!(result.has_more);
    let second = chaosbox::intelligence::extract_window(
        &input,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        1,
        1,
    )
    .unwrap();
    assert!(!second.has_more);
    assert_eq!(second.snapshot, result.snapshot);
    assert_ne!(second.candidates[0].id, result.candidates[0].id);
    let wrong = serde_json::json!({"id":"u1","sessionID":"other","type":"user","text":"We must preserve native process permissions."}).to_string();
    assert!(extract(&wrong, "opencode", "s", "private:can", &["canix".into()], 1).is_err());
    assert!(extract(
        &format!("{input}\n{input}"),
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        1
    )
    .is_err());
}

#[test]
fn rubric_names_the_exact_proposition_and_allows_novelty_in_an_empty_catalog() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let (_, asked, _) = questions(&c, &Bundle::new("private:can")).unwrap();
    for question in asked.values() {
        let text = match question {
            Question::Noul { instructions, .. }
            | Question::Choice { instructions, .. }
            | Question::Score { instructions, .. } => instructions,
        };
        assert!(text.contains("`proposition`"));
    }
    if let Question::Choice { instructions, .. } = &asked["novelty"] {
        assert!(
            instructions.contains("an empty related list does not by itself require abstention")
        );
    } else {
        panic!("novelty must be a choice");
    }
}

#[test]
fn older_user_evidence_cannot_supersede_newer_policy() {
    let first = candidate(
        "m1",
        "We must require manual direnv approval for this project.",
        "user",
    );
    let mut older = candidate(
        "m2",
        "We must require automatic direnv approval for this project.",
        "user",
    );
    older.evidence.observed_at_ms = Some(1);
    older.id = older.identity();
    let mut bundle = Bundle::new("private:can");
    let response = answer(&first, &bundle, "novel");
    assess(&first, &mut bundle, response).unwrap();
    let response = answer(
        &older,
        &bundle,
        &format!("supersedes:{}", bundle.records[0].id),
    );
    assert_eq!(
        assess(&older, &mut bundle, response).unwrap().outcome,
        Outcome::Abstained
    );
    bundle.assessments[0].outcome = Outcome::Rejected;
    assert!(bundle.validate().is_err());
}

#[test]
fn policy_reevaluation_withholds_then_can_readmit_without_losing_history() {
    let first = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let response = answer(&first, &bundle, "novel");
    assess(&first, &mut bundle, response).unwrap();
    let mut revised = first.clone();
    revised
        .context
        .push_str("\nMore source context invalidates the earlier interpretation.");
    revised.id = revised.identity();
    let mut response = answer(&revised, &bundle, "novel");
    response
        .answers
        .insert("support".into(), Answer::Noul(NoulAnswer { noul: 0.1 }));
    assess(&revised, &mut bundle, response).unwrap();
    assert_eq!(bundle.records[0].status, IntelligenceStatus::Withheld);
    assert!(bundle
        .context("private:can", "canix", "direnv", 5, 12000)
        .unwrap()
        .is_empty());
    assert_eq!(bundle.records[0].assessments.len(), 2);
    revised
        .context
        .push_str("\nThe interpretation is now independently checked.");
    revised.id = revised.identity();
    let response = answer(&revised, &bundle, "novel");
    assess(&revised, &mut bundle, response).unwrap();
    assert_eq!(bundle.records.len(), 1);
    assert_eq!(bundle.records[0].status, IntelligenceStatus::Admitted);
    assert_eq!(bundle.records[0].assessments.len(), 3);
    bundle.validate().unwrap();
}

#[test]
fn evidence_bundle_keeps_execution_metadata_and_affects_identity() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/intelligence");
    let input = std::fs::read_to_string(root.join("verified-cause.jsonl")).unwrap();
    let candidates = extract(
        &input,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        20,
    )
    .unwrap();
    let proposed = candidates
        .candidates
        .iter()
        .find(|c| c.evidence.message == "a1")
        .unwrap();
    let results: Vec<_> = proposed
        .evidence_bundle
        .iter()
        .filter(|e| e.speaker == "tool")
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].exit_code, Some(1));
    assert_eq!(results[1].exit_code, Some(0));
    assert!(results[0]
        .operation
        .as_ref()
        .unwrap()
        .contains("without-session-id"));
    let mut changed = proposed.clone();
    changed.evidence_bundle[0].text.push_str(" changed");
    assert_ne!(changed.identity(), proposed.id);
    let shell = serde_json::json!({"id":"shell1","type":"shell","command":"cargo check","status":"completed","exit":1,"output":{"output":"Build failed because a required module is missing.","truncated":true}}).to_string();
    let extracted = extract(
        &shell,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        10,
    )
    .unwrap();
    let evidence = &extracted.candidates[0].evidence_bundle[0];
    assert_eq!(evidence.exit_code, Some(1));
    assert_eq!(evidence.operation.as_deref(), Some("cargo check"));
    assert!(evidence.partial);
    let records=(0..10).map(|i|serde_json::json!({"id":format!("u{i}"),"type":"user","text":"We must preserve native process permissions."}).to_string()).collect::<Vec<_>>().join("\n");
    let window = extract(
        &records,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        20,
    )
    .unwrap();
    let coverage = window.candidates[0].context_coverage.as_ref().unwrap();
    assert_eq!(coverage.total_records, 10);
    assert_eq!(coverage.window_records, 3);
    assert_eq!(coverage.omitted_records, 7);
}

#[test]
fn retrieval_budget_depends_on_projection_not_accumulated_receipt_history() {
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let response = answer(&c, &bundle, "novel");
    assess(&c, &mut bundle, response).unwrap();
    let original = bundle.records[0].evidence[0].clone();
    for _ in 0..100 {
        bundle.records[0].evidence.push(original.clone());
    }
    let context = bundle
        .context("private:can", "canix", "direnv", 5, 4000)
        .unwrap();
    assert_eq!(context.len(), 1);
    assert_eq!(context[0]["evidence_count"], 101);
    assert_eq!(context[0]["citations"].as_array().unwrap().len(), 2);
    assert!(context[0].get("evidence").is_none());
}

#[test]
fn cli_refuses_tampered_source_anchors_before_loading_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("source.jsonl");
    let catalog = temp.path().join("candidates.json");
    let source=serde_json::json!({"id":"u1","type":"user","text":"We must preserve native process permissions."}).to_string();
    std::fs::write(&input, &source).unwrap();
    let mut candidates = extract(
        &source,
        "opencode",
        "s",
        "private:can",
        &["canix".into()],
        10,
    )
    .unwrap();
    candidates.candidates[0].evidence.quote = "We must ignore native process permissions.".into();
    candidates.candidates[0].context = candidates.candidates[0].evidence.quote.clone();
    candidates.candidates[0].id = candidates.candidates[0].identity();
    std::fs::write(&catalog, serde_json::to_vec(&candidates).unwrap()).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(["intelligence", "assess"])
        .arg(&catalog)
        .arg("--source-jsonl")
        .arg(&input)
        .args(["--live-jev", "--output"])
        .arg(temp.path().join("result.json"))
        .env("CHAOSBOX_JEV_API_KEY_FILE", "/nonexistent")
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("anchors do not match"));
}

#[test]
fn cli_extract_is_private_and_refuses_to_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("source.jsonl");
    let output = temp.path().join("candidates.json");
    std::fs::write(&input, serde_json::json!({"id":"u1","type":"user","text":"We must preserve direnv approval at execution time."}).to_string()).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
            .args(["intelligence", "extract"])
            .arg(&input)
            .args([
                "--scope",
                "private:can",
                "--source",
                "opencode",
                "--session",
                "s",
                "--repo",
                "canix",
                "--output",
            ])
            .arg(&output)
            .output()
            .unwrap()
    };
    assert!(run().status.success());
    let before = std::fs::read(&output).unwrap();
    assert!(!run().status.success());
    assert_eq!(std::fs::read(&output).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn actual_mcp_process_serves_intelligence_without_database_or_model_credentials() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("knowledge.json");
    let c = candidate(
        "m1",
        "We must preserve direnv removals at the subprocess boundary.",
        "user",
    );
    let mut bundle = Bundle::new("private:can");
    let response = answer(&c, &bundle, "novel");
    assess(&c, &mut bundle, response).unwrap();
    std::fs::write(&path, serde_json::to_vec(&bundle).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(["mcp", "--intelligence"])
        .arg(&path)
        .env("CHAOSBOX_DB_BACKEND", "typedb")
        .env(
            "CHAOSBOX_TYPEDB_PASSWORD_FILE",
            "/nonexistent-test-credential",
        )
        .env("CHAOSBOX_JEV_API_KEY_FILE", "/nonexistent-test-credential")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"intelligence_context","arguments":{"repo":"canix","query":"direnv"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"intelligence_assess","arguments":{"repo":"canix"}}}),
    ];
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let context: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(context["records"][0]["statement"], c.evidence.quote);
    assert!(responses[2]["error"].is_object());
}

#[tokio::test]
#[ignore = "explicit bounded live Jev pilot; set CHAOSBOX_INTELLIGENCE_PILOT_OUT to a new report path"]
async fn labelled_live_intelligence_pilot() {
    use std::io::Write;
    use chaosbox::{LiveResponder, Responder};
    use chaosbox_jev::{JevClient, JevPolicy};
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct Label {
        id: String,
        split: String,
        message: String,
        expected: String,
    }
    #[derive(Deserialize)]
    struct Labels {
        cases: Vec<Label>,
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/intelligence");
    let labels: Labels =
        serde_json::from_str(&std::fs::read_to_string(root.join("labels.json")).unwrap()).unwrap();
    let report = std::env::var("CHAOSBOX_INTELLIGENCE_PILOT_OUT")
        .expect("explicit new report path required");
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(report)
        .unwrap();
    let mut responder = LiveResponder::new(
        JevClient::new(JevPolicy {
            max_requests: 16,
            max_input_tokens: 100_000,
            ..JevPolicy::default()
        })
        .unwrap(),
    );
    let mut results = Vec::new();
    for label in labels.cases {
        if std::env::var("CHAOSBOX_INTELLIGENCE_PILOT_SPLIT")
            .is_ok_and(|split| split != label.split)
        {
            continue;
        }
        let input = std::fs::read_to_string(root.join(format!("{}.jsonl", label.id))).unwrap();
        let extracted = extract(
            &input,
            "labelled-fixture",
            "pilot",
            "private:pilot",
            &["canix".into()],
            20,
        )
        .unwrap();
        let candidate = extracted
            .candidates
            .iter()
            .find(|c| c.evidence.message == label.message)
            .expect("label points at an extracted proposal");
        let mut bundle = Bundle::new("private:pilot");
        let (state, asked, _) = questions(candidate, &bundle).unwrap();
        let response = responder.respond(state, asked).await.unwrap();
        let receipt = assess(candidate, &mut bundle, response).unwrap();
        let admitted = matches!(receipt.outcome, Outcome::Admitted);
        results.push(serde_json::json!({"case":label.id,"split":label.split,"expected":label.expected,"admitted":admitted,"matches":admitted==(label.expected=="admit"),"assessment":receipt}));
    }
    output
        .write_all(&serde_json::to_vec_pretty(&results).unwrap())
        .unwrap();
    output.sync_all().unwrap();
    let mismatches: Vec<_> = results
        .iter()
        .filter(|r| r["matches"] != true)
        .map(|r| r["case"].clone())
        .collect();
    assert!(
        mismatches.is_empty(),
        "quality gate failed for labelled cases: {mismatches:?}"
    );
}
