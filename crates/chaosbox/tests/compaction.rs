//! Crash, custody and pruning invariants on real private SQLite journals.
use chaosbox::compaction::{Capture, Journal};
use serde_json::{json, Value};

fn capture(records: Vec<Value>) -> Capture {
    Capture {
        version: 1,
        scope: "private:can".into(),
        repo: "chaosbox".into(),
        source: "opencode".into(),
        session: "ses_test".into(),
        records,
    }
}

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

fn records() -> Vec<Value> {
    vec![
        json!({"id":"msg_user","type":"user","time":{"created":1},
        "prompt":{"parts":[{"type":"text","text":"Keep the user's exact requirements, including λ."}]}}),
        json!({"id":"msg_tool","type":"assistant","time":{"created":2,"completed":3},
        "parts":[{"type":"text","text":"The next step is still pending."},
        {"type":"tool","id":"call_test","name":"shell","state":{"status":"completed",
        "input":{"command":"cargo test"},"metadata":{"exitCode":1},
        "content":[{"type":"text","text":"failed\n".repeat(10000)}]}}]}),
    ]
}

#[test]
fn custody_replays_after_restart_and_outputs_are_recoverable() {
    let dir = private_dir();
    let request = capture(records());
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let first = journal.capture(&request).unwrap();
    let plan = journal.plan(&first.id, 8000).unwrap();
    assert!(plan.summary.contains("including λ"));
    assert!(plan.summary.contains("pending"));
    assert!(plan.summary.contains("exitCode"));
    assert!(!plan.summary.contains(&"failed\n".repeat(10000)));
    assert_eq!(plan.externalized.len(), 1);
    let archived = journal.evidence(&plan.externalized[0]).unwrap();
    assert_eq!(archived, request.records[1]);
    drop(journal);
    let mut reopened = Journal::open(dir.path(), "private:can").unwrap();
    assert_eq!(reopened.capture(&request).unwrap().id, first.id);
    assert_eq!(
        reopened.plan(&first.id, 8000).unwrap().summary,
        plan.summary
    );
    assert_eq!(reopened.pending().unwrap().len(), 1);
}

#[test]
fn refuses_unsafe_reductions_and_cross_scope_reads() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    assert!(
        journal
            .capture(&capture(vec![json!({"id":"msg_x","type":"assistant",
        "time":{"created":1},"parts":[]})]))
            .is_err()
    );
    let receipt = journal.capture(&capture(records())).unwrap();
    assert!(journal.plan(&receipt.id, 512).is_err());
    assert!(Journal::open(dir.path(), "private:someone-else").is_err());
    let mut cross = capture(records());
    cross.records[0]["sessionID"] = json!("ses_other");
    assert!(journal.capture(&cross).is_err());
}

#[test]
fn previous_compactions_never_become_evidence_and_variants_remain_archived() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let mut request = capture(records());
    let before = journal.capture(&request).unwrap();
    request.records.push(
        json!({"id":"msg_summary","type":"compaction","status":"completed",
        "summary":"Invented success should never be independent evidence."}),
    );
    request.records[1]["parts"][0]["text"] = json!("The next step is now blocked.");
    let after = journal.capture(&request).unwrap();
    assert_ne!(before.id, after.id);
    assert!(
        !journal
            .plan(&after.id, 8000)
            .unwrap()
            .summary
            .contains("Invented success")
    );
    assert!(
        journal
            .plan(&before.id, 8000)
            .unwrap()
            .summary
            .contains("still pending")
    );
}

#[cfg(unix)]
#[test]
fn refuses_symlink_journals() {
    use std::os::unix::fs::symlink;
    let dir = private_dir();
    let other = tempfile::NamedTempFile::new().unwrap();
    symlink(other.path(), dir.path().join("memory.sqlite")).unwrap();
    assert!(Journal::open(dir.path(), "private:can").is_err());
}

struct Decisions {
    calls: usize,
    fail: bool,
}

#[async_trait::async_trait]
impl chaosbox::Responder for Decisions {
    async fn respond(
        &mut self,
        state: Value,
        asked: std::collections::BTreeMap<String, chaosbox_jev::Question>,
    ) -> Result<chaosbox_jev::SystemOneResponse, String> {
        use chaosbox_jev::{Answer, ChoiceAnswer, NoulAnswer, Question};
        self.calls += 1;
        if self.fail {
            return Err("unavailable".into());
        }
        let answers = asked
            .into_iter()
            .map(|(id, q)| {
                let answer = match q {
                    Question::Noul { .. } => Answer::Noul(NoulAnswer {
                        noul: if state["speaker"] == "user" {
                            0.99
                        } else {
                            0.01
                        },
                    }),
                    Question::Choice { criteria, .. } => {
                        let choice = match id.as_str() {
                            "kind" => "constraint".to_owned(),
                            "novelty" => state["related"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .find(|r| r["statement"] == state["proposition"])
                                .map_or_else(
                                    || "novel".into(),
                                    |r| format!("duplicate:{}", r["id"].as_str().unwrap()),
                                ),
                            _ => "irrelevant".to_owned(),
                        };
                        let probabilities = criteria
                            .keys()
                            .map(|k| {
                                (
                                    k.clone(),
                                    if k == &choice {
                                        0.97
                                    } else {
                                        0.03 / f64::from(u32::try_from(criteria.len() - 1).unwrap())
                                    },
                                )
                            })
                            .collect();
                        Answer::Choice(ChoiceAnswer {
                            choice,
                            probabilities,
                            confidence: 0.99,
                        })
                    }
                    Question::Score { .. } => unreachable!(),
                };
                (id, answer)
            })
            .collect();
        Ok(chaosbox_jev::SystemOneResponse {
            model: chaosbox_jev::JEV_MODEL_PINNED.into(),
            answers,
            usage: chaosbox_jev::Usage {
                input_tokens: 50,
                output_tokens: 0,
            },
        })
    }
}

#[tokio::test]
async fn admission_is_independent_of_pruning_and_receipts_are_anchor_bound() {
    use chaosbox::compaction::Budget;
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let mut source = capture(vec![
        json!({"id":"msg_u","type":"user","text":"Καθορισμένη επιλογή για αυτό το έργο.","time":{"created":1}}),
        json!({"id":"msg_a","type":"assistant","parts":[{"type":"text","text":"An incidental remark unrelated to the active objective."}],"time":{"created":2,"completed":3}}),
    ]);
    let receipt = journal.capture(&source).unwrap();
    let mut responder = Decisions {
        calls: 0,
        fail: false,
    };
    let budget = Budget {
        requests: 20,
        input_tokens: 1_000_000,
        retry: false,
    };
    assert_eq!(
        journal
            .assess_pending(dir.path(), &mut responder, budget)
            .await
            .unwrap(),
        1
    );
    assert_eq!(journal.bundle().unwrap().records.len(), 1);
    assert_eq!(journal.bundle().unwrap().assessments.len(), 2);
    assert_eq!(journal.plan(&receipt.id, 8000).unwrap().omitted.len(), 1);
    assert_eq!(
        journal
            .assess_pending(dir.path(), &mut responder, budget)
            .await
            .unwrap(),
        0
    );
    assert_eq!(responder.calls, 3);
    assert_eq!(
        journal
            .bundle()
            .unwrap()
            .context("private:can", "chaosbox", "Καθορισμένη", 5, 12000)
            .unwrap()
            .len(),
        1
    );
    source.records.push(json!({"id":"msg_new","type":"user","text":"Reconsider the incidental remark for the new objective.","time":{"created":4}}));
    let new = journal.capture(&source).unwrap();
    assert_eq!(
        journal.plan(&new.id, 8000).unwrap().omitted,
        Vec::<String>::new()
    );
    // Original normalized evidence remains recoverable by the database citation.
    let snapshot = journal.bundle().unwrap().records[0].evidence[0]
        .snapshot
        .clone();
    assert!(
        journal.evidence(&snapshot).unwrap()["source_jsonl"]
            .as_str()
            .unwrap()
            .contains("Καθορισμένη")
    );
}

#[tokio::test]
async fn failed_attempts_and_spending_survive_restart() {
    use chaosbox::compaction::Budget;
    let dir = private_dir();
    let source = capture(vec![
        json!({"id":"msg_u","type":"user","text":"Preserve the exact repository-specific operating decision.","time":{"created":1}}),
    ]);
    let budget = Budget {
        requests: 1,
        input_tokens: 1_000_000,
        retry: false,
    };
    let mut responder = Decisions {
        calls: 0,
        fail: true,
    };
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let receipt = journal.capture(&source).unwrap();
    assert!(
        journal
            .assess_pending(dir.path(), &mut responder, budget)
            .await
            .is_err()
    );
    assert!(journal.plan(&receipt.id, 8000).is_ok());
    drop(journal);
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    assert!(
        journal
            .assess_pending(dir.path(), &mut responder, budget)
            .await
            .unwrap_err()
            .contains("--retry")
    );
    assert!(
        journal
            .assess_pending(
                dir.path(),
                &mut responder,
                Budget {
                    retry: true,
                    ..budget
                }
            )
            .await
            .unwrap_err()
            .contains("budget")
    );
    assert_eq!(responder.calls, 1);
    assert_eq!(journal.pending().unwrap().len(), 1);
}

#[test]
fn bounded_native_output_requires_matching_pre_bounding_custody() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let mut request = capture(records());
    request.records[1]["parts"][1]["state"]["metadata"]["truncated"] = json!(true);
    let receipt = journal.capture(&request).unwrap();
    assert!(
        journal
            .plan(&receipt.id, 8000)
            .unwrap_err()
            .contains("pre-bounding")
    );
    let tool = capture(vec![
        json!({"id":"msg_tool:call_test","type":"tool-result","messageID":"msg_tool","callID":"call_test",
        "tool":"shell","input":{"command":"cargo test"},"status":"completed","result":{"content":[{"type":"text","text":"complete result"}]}}),
    ]);
    journal.capture(&tool).unwrap();
    assert!(
        journal
            .plan(&receipt.id, 8000)
            .unwrap()
            .summary
            .contains("archive_hash")
    );
    let mut wrong = tool;
    wrong.records[0]["input"] = json!({"command":"different command"});
    assert!(journal.capture(&wrong).is_err());
}

#[test]
fn native_job_notifications_and_instruction_updates_remain_protected() {
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let mut source = capture(records());
    source.records.extend([
        json!({"id":"msg_job","type":"synthetic","text":"Background test finished with failures; repair is pending."}),
        json!({"id":"msg_instruction","type":"system","text":"New instruction: use the private database only."}),
        json!({"id":"msg_idle","type":"idle","outcome":"failed"}),
        json!({"id":"msg_model","type":"model-switched","model":{"id":"current"}}),
    ]);
    let id = journal.capture(&source).unwrap().id;
    let plan = journal.plan(&id, 10000).unwrap();
    assert!(plan.summary.contains("repair is pending"));
    assert!(plan.summary.contains("use the private database only"));
    assert!(plan.summary.contains("model-switched"));
    source.records[1]["parts"][1]["state"]["content"] =
        json!([{"type":"file","uri":"file:///ephemeral/image.png","mime":"image/png"}]);
    let id = journal.capture(&source).unwrap().id;
    assert!(
        journal
            .plan(&id, 10000)
            .unwrap_err()
            .contains("attachment bytes")
    );
}

#[tokio::test]
async fn early_string_results_reconcile_to_one_native_tool_occurrence() {
    use chaosbox::compaction::Budget;
    let dir = private_dir();
    let mut journal = Journal::open(dir.path(), "private:can").unwrap();
    let result = "Complete evidence: preserve the original operation outcome.";
    let early = capture(vec![
        json!({"id":"msg_tool:call_test","type":"tool-result","messageID":"msg_tool","callID":"call_test",
        "tool":"shell","input":{"command":"cargo test"},"status":"completed","result":{"content":result,"metadata":{"exitCode":1}}}),
    ]);
    journal.capture(&early).unwrap();
    let mut source = capture(records());
    source.records[1]["parts"][1]["state"]["content"] =
        json!([{"type":"text","text":"bounded preview"}]);
    source.records[1]["parts"][1]["state"]["metadata"]["truncated"] = json!(true);
    journal.capture(&source).unwrap();
    let mut responder = Decisions {
        calls: 0,
        fail: false,
    };
    journal
        .assess_pending(
            dir.path(),
            &mut responder,
            Budget {
                requests: 20,
                input_tokens: 1_000_000,
                retry: false,
            },
        )
        .await
        .unwrap();
    let bundle = journal.bundle().unwrap();
    assert_eq!(
        bundle
            .assessments
            .iter()
            .filter(|a| a.state.as_ref().is_some_and(|s| s["proposition"] == result))
            .count(),
        1
    );
    assert!(!bundle.assessments.iter().any(|a| {
        a.state
            .as_ref()
            .is_some_and(|s| s["proposition"] == "bounded preview")
    }));
}

#[tokio::test]
#[ignore = "requires the disposable TypeDB gate, explicit credentials and migrated schema"]
async fn private_cross_session_memory_publishes_and_recovers_from_database() {
    use chaosbox::compaction::Budget;
    use chaosbox_typedb::store::{TypeDbStore, TypeDbConfig};
    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    let mut store = TypeDbStore::new(config.clone());
    let dir = private_dir();
    let scope = format!("private:compaction-test-{}", std::process::id());
    let mut journal = Journal::open(dir.path(), &scope).unwrap();
    let mut source = capture(vec![
        json!({"id":"msg_policy","type":"user","text":"Preserve repository-specific decisions with exact provenance.","time":{"created":1}}),
    ]);
    source.scope = scope.clone();
    journal.capture(&source).unwrap();
    let mut responder = Decisions {
        calls: 0,
        fail: false,
    };
    let budget = Budget {
        requests: 20,
        input_tokens: 1_000_000,
        retry: false,
    };
    journal
        .assess_pending(dir.path(), &mut responder, budget)
        .await
        .unwrap();
    let id = journal
        .publish_pending(dir.path(), &mut store)
        .await
        .unwrap();
    // Simulates a fresh session/process using the same authoritative database.
    let mut reader = TypeDbStore::new(config);
    let (pin, raw) = reader.knowledge(&scope).await.unwrap().unwrap();
    assert_eq!(pin, id);
    let published: chaosbox::intelligence::Bundle = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        published
            .context(&scope, "chaosbox", "provenance", 5, 12000)
            .unwrap()
            .len(),
        1
    );
    assert!(
        published
            .context("private:other", "chaosbox", "provenance", 5, 12000)
            .is_err()
    );
    assert_eq!(
        published
            .context(&scope, "other-repo", "provenance", 5, 12000)
            .unwrap(),
        Vec::<serde_json::Value>::new()
    );
    source.session = "ses_second".into();
    source.records[0]["id"] = json!("msg_second");
    source.records[0]["time"]["created"] = json!(2);
    journal.capture(&source).unwrap();
    journal
        .assess_pending(dir.path(), &mut responder, budget)
        .await
        .unwrap();
    let next = journal
        .publish_pending(dir.path(), &mut store)
        .await
        .unwrap();
    assert_ne!(next, id);
    assert_eq!(
        journal
            .publish_pending(dir.path(), &mut store)
            .await
            .unwrap(),
        next
    );
    let (_, raw) = reader.knowledge(&scope).await.unwrap().unwrap();
    let bundle: chaosbox::intelligence::Bundle = serde_json::from_str(&raw).unwrap();
    bundle.validate().unwrap();
    assert_eq!(bundle.records.len(), 1);
    assert_eq!(bundle.records[0].evidence.len(), 2);
    assert_eq!(bundle.assessments.len(), 2);
}
