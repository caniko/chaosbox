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
            Outcome::Rejected
        );
        assert!(bundle.records.is_empty());
        assert_eq!(bundle.assessments.len(), 1);
    }
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
