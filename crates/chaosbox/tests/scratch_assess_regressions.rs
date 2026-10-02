//! Offline regressions for assessment custody, protocol validation and recovery.
use std::{collections::BTreeMap, path::PathBuf};

use chaosbox::{
    scratch::{assessment::Budget, cli, Ledger, Request},
    Responder,
};
use chaosbox_jev::{Question, SystemOneResponse, JEV_MODEL_PINNED};
use serde_json::{json, Value};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    work: PathBuf,
    ledger: Ledger,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("scratch");
        let path = root.join("work");
        let work = temp.path().join("custody");
        std::fs::create_dir_all(&path).unwrap();
        let ledger = Ledger::open(&work, "private:test").unwrap();
        let mut fixture = Self {
            _temp: temp,
            root,
            path,
            work,
            ledger,
        };
        fixture.record(json!([
            {"kind":"begin","id":"begin","invocation":"call","session":"ses","message":"msg","tool":"shell","repo":"repo","command":"cargo test regression","cwd":fixture.path,
             "sources":[{"record":{"id":"user","type":"user","text":"Run the regression test."},"pointer":"/text"}]},
            {"kind":"observe","id":"observe","path":fixture.path,"identity":fixture.identity(),"present":true,"activity":true,"owners":["call"]},
            {"kind":"end","id":"end","invocation":"call","outcome":"exited","exit":0},
            {"kind":"coverage","id":"coverage","observer":"test","healthy":true,"detail":"watching"}
        ]));
        fixture
    }

    fn identity(&self) -> chaosbox::scratch::Identity {
        chaosbox::scratch::identity(&self.path, &self.root).unwrap()
    }

    fn record(&mut self, events: Value) {
        let mut input = json!({"version":1,"scope":"private:test","host":"test","root":self.root});
        input["events"] = events;
        self.ledger
            .record(&serde_json::from_value::<Request>(input).unwrap())
            .unwrap();
    }

    fn view(&self) -> Value {
        self.ledger
            .query("test", &self.root, std::slice::from_ref(&self.path), 100)
            .unwrap()
    }

    async fn assess(&mut self, judge: &mut Judge, budget: Budget) -> Result<Value, String> {
        self.ledger
            .assess(
                judge,
                "test",
                &self.root,
                std::slice::from_ref(&self.path),
                budget,
            )
            .await
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.work.join("scratch.sqlite")).unwrap()
    }
}

#[derive(Default)]
struct Judge {
    calls: usize,
    fault: Option<&'static str>,
    fail_call: usize,
    pending_call: usize,
    pending_started: Option<tokio::sync::oneshot::Sender<()>>,
    satisfied: bool,
    uncertain_support: bool,
}

#[async_trait::async_trait]
impl Responder for Judge {
    async fn respond(
        &mut self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        self.calls += 1;
        if self.calls == self.pending_call {
            if let Some(started) = self.pending_started.take() {
                started.send(()).unwrap();
            }
            std::future::pending::<()>().await;
        }
        if self.calls == self.fail_call {
            return Err("completion service offline".into());
        }
        let execution = state["support"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "execution")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let mut answers = serde_json::Map::new();
        for (id, question) in &questions {
            let Question::Choice { criteria, .. } = question else {
                panic!("choice required")
            };
            let selected = if id == "purpose" {
                "s0"
            } else if id.starts_with("role_") {
                "obligation"
            } else if id.starts_with("status_") {
                if self.satisfied {
                    "satisfied"
                } else {
                    "outstanding"
                }
            } else if id.starts_with("support_") {
                execution
            } else {
                "high"
            };
            let probabilities: BTreeMap<_, _> = criteria
                .keys()
                .map(|key| (key.clone(), f64::from(key == selected)))
                .collect();
            answers.insert(id.clone(), json!({"type":"choice","choice":selected,"confidence":1.0,"probabilities":probabilities}));
        }
        let mut response = json!({"model":JEV_MODEL_PINNED,"usage":{"input_tokens":1,"output_tokens":1},"answers":answers});
        if self.uncertain_support && questions.contains_key("support_0") {
            response["answers"]["support_0"]["confidence"] = json!(0.79);
        }
        if questions.contains_key("purpose") {
            match self.fault {
                Some("model") => response["model"] = json!("substituted-model"),
                Some("missing-answer") => {
                    response["answers"]
                        .as_object_mut()
                        .unwrap()
                        .remove("purpose");
                }
                Some("extra-answer") => {
                    response["answers"]["unasked"] = response["answers"]["purpose"].clone();
                }
                Some("invalid-choice") => response["answers"]["purpose"]["choice"] = json!("s999"),
                Some("missing-probability") => {
                    response["answers"]["purpose"]["probabilities"]
                        .as_object_mut()
                        .unwrap()
                        .remove("unknown");
                }
                Some("non-normalized") => {
                    response["answers"]["purpose"]["probabilities"]["unknown"] = json!(0.5);
                }
                Some("not-argmax") => {
                    response["answers"]["purpose"]["probabilities"]["s0"] = json!(0.2);
                    response["answers"]["purpose"]["probabilities"]["unknown"] = json!(0.8);
                }
                Some("low-probability") => {
                    response["answers"]["role_0"]["probabilities"]["obligation"] = json!(0.79);
                    response["answers"]["role_0"]["probabilities"]["unknown"] = json!(0.21);
                }
                Some("purpose-only") => {
                    response["answers"]["role_0"]["choice"] = json!("purpose");
                    response["answers"]["role_0"]["probabilities"]["obligation"] = json!(0.0);
                    response["answers"]["role_0"]["probabilities"]["purpose"] = json!(1.0);
                }
                None => {}
                Some(fault) => panic!("unhandled fault {fault}"),
            }
        }
        Ok(serde_json::from_value(response).unwrap())
    }
}

fn budget() -> Budget {
    Budget {
        requests: 20,
        input_tokens: 1_000_000,
        retry: false,
    }
}

#[tokio::test]
async fn malformed_or_substituted_responses_are_durable_failures_and_keep_holds() {
    for fault in [
        "model",
        "missing-answer",
        "extra-answer",
        "invalid-choice",
        "missing-probability",
        "non-normalized",
        "not-argmax",
    ] {
        let mut fixture = Fixture::new();
        let mut judge = Judge {
            fault: Some(fault),
            ..Default::default()
        };
        assert!(
            fixture.assess(&mut judge, budget()).await.is_err(),
            "{fault}"
        );
        let view = fixture.view();
        assert_eq!(view["items"][0]["blocked"], true, "{fault}");
        assert_eq!(
            view["items"][0]["entries"][0]["assessment"],
            Value::Null,
            "{fault}"
        );
        assert_eq!(
            view["items"][0]["entries"][0]["assessment_state"], "failed-or-interrupted",
            "{fault}"
        );
        let spending = fixture.ledger.assessment_status().unwrap();
        assert_eq!(spending["states"][0]["status"], "failed", "{fault}");
        assert_eq!(spending["states"][0]["requests"], 1, "{fault}");
        assert!(
            spending["states"][0]["reserved_input_tokens"]
                .as_u64()
                .unwrap()
                > 4096
        );
        judge.fault = None;
        assert!(fixture.assess(&mut judge, budget()).await.is_err());
        assert_eq!(judge.calls, 1, "{fault} must require explicit retry");
    }
}

#[tokio::test]
async fn completion_stage_retry_reuses_classification_and_preserves_failed_spending() {
    let mut fixture = Fixture::new();
    let mut judge = Judge {
        fail_call: 2,
        ..Default::default()
    };
    assert!(fixture.assess(&mut judge, budget()).await.is_err());
    assert_eq!(judge.calls, 2);
    let work = fixture.work.clone();
    fixture.ledger = Ledger::open(&work, "private:test").unwrap();
    let cap = Budget {
        requests: 3,
        retry: true,
        ..budget()
    };
    fixture.assess(&mut judge, cap).await.unwrap();
    assert_eq!(
        judge.calls, 3,
        "only the failed completion stage is dispatched again"
    );
    assert_eq!(
        fixture.view()["items"][0]["entries"][0]["assessment_state"],
        "current"
    );
    let status = fixture.ledger.assessment_status().unwrap();
    assert_eq!(status["states"][0]["status"], "failed");
    assert_eq!(status["states"][0]["requests"], 1);
    assert_eq!(status["states"][1]["requests"], 2);
}

#[tokio::test]
async fn cancelled_dispatch_survives_restart_and_requires_explicit_retry() {
    let mut fixture = Fixture::new();
    let (started, dispatched) = tokio::sync::oneshot::channel();
    let mut judge = Judge {
        pending_call: 2,
        pending_started: Some(started),
        ..Default::default()
    };
    {
        let assessment = fixture.assess(&mut judge, budget());
        tokio::pin!(assessment);
        tokio::select! {
            result = &mut assessment => panic!("assessment unexpectedly completed: {result:?}"),
            result = dispatched => result.unwrap(),
            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("completion dispatch never started"),
        }
        // Drop only after the completion dispatch reservation is durably written.
    }
    let work = fixture.work.clone();
    fixture.ledger = Ledger::open(&work, "private:test").unwrap();
    let status = fixture.ledger.assessment_status().unwrap();
    assert_eq!(status["states"][0]["status"], "dispatching");
    assert_eq!(status["states"][0]["requests"], 1);
    assert!(fixture.assess(&mut judge, budget()).await.is_err());
    assert_eq!(
        judge.calls, 2,
        "interrupted completion must not silently retry"
    );
    assert!(fixture
        .assess(
            &mut judge,
            Budget {
                requests: 2,
                retry: true,
                ..budget()
            }
        )
        .await
        .is_err());
    assert_eq!(
        judge.calls, 2,
        "interrupted dispatch still consumes its reservation"
    );
    fixture
        .assess(
            &mut judge,
            Budget {
                requests: 3,
                retry: true,
                ..budget()
            },
        )
        .await
        .unwrap();
    assert_eq!(judge.calls, 3, "classification survives cancellation");
}

#[tokio::test]
async fn corrupted_success_cache_is_revalidated_before_completion_retry() {
    for corrupt in ["not json", "substituted-model"] {
        let mut fixture = Fixture::new();
        let mut judge = Judge {
            fail_call: 2,
            ..Default::default()
        };
        assert!(fixture.assess(&mut judge, budget()).await.is_err());
        let db = fixture.db();
        let cached: String = db
            .query_row(
                "SELECT response FROM scratch_attempts WHERE status='success'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let damaged = if corrupt == "not json" {
            corrupt.to_owned()
        } else {
            let mut response: Value = serde_json::from_str(&cached).unwrap();
            response["model"] = json!(corrupt);
            response.to_string()
        };
        db.execute(
            "UPDATE scratch_attempts SET response=?1 WHERE status='success'",
            [&damaged],
        )
        .unwrap();
        assert!(fixture
            .assess(
                &mut judge,
                Budget {
                    retry: true,
                    ..budget()
                }
            )
            .await
            .is_err());
        assert_eq!(
            judge.calls, 2,
            "damaged cache cannot start completion inference"
        );
        assert_eq!(
            fixture.view()["items"][0]["entries"][0]["assessment"],
            Value::Null
        );
        assert_eq!(fixture.view()["items"][0]["blocked"], true);
    }
}

#[tokio::test]
async fn new_context_annotation_and_link_each_invalidate_and_refresh_evidence() {
    for change in ["context", "annotation", "link"] {
        let mut fixture = Fixture::new();
        let mut judge = Judge::default();
        fixture.assess(&mut judge, budget()).await.unwrap();
        let old = fixture.view()["items"][0]["entries"][0]["assessment"]["evidence_digest"].clone();
        let event = match change {
            "context" => {
                json!({"kind":"context","id":"context","invocation":"call","sources":[{"record":{"id":"later","type":"assistant","content":[{"type":"text","text":"The patch still needs review."}]},"pointer":"/content/0/text"}]})
            }
            "annotation" => {
                json!({"kind":"annotation","id":"note","path":fixture.path,"identity":fixture.identity(),"disposition":"needs-finalization","reason":"Review the saved patch."})
            }
            "link" => {
                json!({"kind":"link","id":"link","path":fixture.path,"identity":fixture.identity(),"category":"commit","reference":"commit:abc","description":"Integration asserted"})
            }
            _ => unreachable!(),
        };
        fixture.record(json!([event]));
        let stale = fixture.view();
        assert_eq!(
            stale["items"][0]["entries"][0]["assessment"]["fresh"], false,
            "{change}"
        );
        assert_eq!(
            stale["items"][0]["entries"][0]["assessment_state"], "pending",
            "{change}"
        );
        assert_eq!(stale["items"][0]["blocked"], true);
        fixture.assess(&mut judge, budget()).await.unwrap();
        let refreshed = fixture.view();
        assert_eq!(
            refreshed["items"][0]["entries"][0]["assessment"]["fresh"],
            true
        );
        assert_ne!(
            refreshed["items"][0]["entries"][0]["assessment"]["evidence_digest"],
            old
        );
        assert_eq!(judge.calls, 4, "{change} invalidates both stages");
    }
}

#[tokio::test]
async fn verified_completion_only_recommends_release_and_new_activity_reopens_it() {
    let mut fixture = Fixture::new();
    let mut judge = Judge {
        satisfied: true,
        ..Default::default()
    };
    fixture.assess(&mut judge, budget()).await.unwrap();
    let view = fixture.view();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(assessment["obligations"][0]["status"], "satisfied");
    assert_eq!(assessment["obligations"][0]["support"]["verified"], true);
    assert_eq!(assessment["release_recommended"], true);
    assert_eq!(
        view["items"][0]["blocked"], true,
        "a recommendation is not cleanup authorization"
    );
    fixture
        .ledger
        .annotate(
            "test",
            &fixture.root,
            &fixture.path,
            "released",
            "Regression test completed; output disposable",
            None,
        )
        .unwrap();
    assert_eq!(fixture.view()["items"][0]["blocked"], false);
    fixture.record(json!([{"kind":"observe","id":"new-activity","path":fixture.path,"identity":fixture.identity(),"present":true,"activity":true,"owners":["call"]}]));
    assert_eq!(fixture.view()["items"][0]["blocked"], true);
    assert_eq!(
        fixture.view()["items"][0]["entries"][0]["disposition"],
        "open"
    );
}

#[tokio::test]
async fn omitted_evidence_prevents_release_even_with_verified_completion() {
    let mut fixture = Fixture::new();
    fixture.record(json!([
        {"kind":"begin","id":"extra-begin","invocation":"extra","session":"ses","message":"msg","tool":"shell","repo":"repo","command":"cargo test regression","cwd":fixture.path,"sources_omitted":1},
        {"kind":"observe","id":"extra-observe","path":fixture.path,"identity":fixture.identity(),"present":true,"activity":true,"owners":["extra"]},
        {"kind":"end","id":"extra-end","invocation":"extra","outcome":"exited","exit":0}
    ]));
    let mut judge = Judge {
        satisfied: true,
        ..Default::default()
    };
    fixture.assess(&mut judge, budget()).await.unwrap();
    let view = fixture.view();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(assessment["obligations"][0]["status"], "satisfied");
    assert_eq!(assessment["omitted"], 1);
    assert_eq!(assessment["release_recommended"], false);
    assert_eq!(view["items"][0]["blocked"], true);
}

#[tokio::test]
async fn uncertain_support_cannot_prove_completion() {
    let mut fixture = Fixture::new();
    let mut judge = Judge {
        satisfied: true,
        uncertain_support: true,
        ..Default::default()
    };
    fixture.assess(&mut judge, budget()).await.unwrap();
    let view = fixture.view();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(assessment["obligations"][0]["status"], "unknown");
    assert_eq!(assessment["obligations"][0]["support"], Value::Null);
    assert_eq!(assessment["release_recommended"], false);
    assert_eq!(
        view["items"][0]["entries"][0]["disposition"],
        "needs-finalization"
    );
}

#[tokio::test]
async fn high_confidence_with_low_selected_probability_retains_unresolved_source() {
    let mut fixture = Fixture::new();
    let mut judge = Judge {
        fault: Some("low-probability"),
        satisfied: true,
        ..Default::default()
    };
    fixture.assess(&mut judge, budget()).await.unwrap();
    let view = fixture.view();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(
        assessment["unresolved_sources"][0]["quote"],
        "Run the regression test."
    );
    assert_eq!(assessment["release_recommended"], false);
    assert_eq!(judge.calls, 1);
}

#[tokio::test]
async fn insufficient_token_budget_never_dispatches_or_reserves_a_request() {
    let mut fixture = Fixture::new();
    let mut judge = Judge::default();
    assert!(fixture
        .assess(
            &mut judge,
            Budget {
                input_tokens: 1,
                ..budget()
            }
        )
        .await
        .is_err());
    assert_eq!(judge.calls, 0);
    assert_eq!(
        fixture.ledger.assessment_status().unwrap()["states"],
        json!([])
    );
    fixture
        .assess(
            &mut judge,
            Budget {
                retry: true,
                ..budget()
            },
        )
        .await
        .unwrap();
    assert_eq!(judge.calls, 2);
}

#[tokio::test]
async fn failed_token_reservations_survive_restart_and_exhaust_a_retry_budget() {
    let mut fixture = Fixture::new();
    let mut judge = Judge {
        fail_call: 2,
        ..Default::default()
    };
    assert!(fixture.assess(&mut judge, budget()).await.is_err());
    let status = fixture.ledger.assessment_status().unwrap();
    let spent: u64 = status["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|state| state["reserved_input_tokens"].as_u64().unwrap())
        .sum();
    let work = fixture.work.clone();
    fixture.ledger = Ledger::open(&work, "private:test").unwrap();
    assert!(fixture
        .assess(
            &mut judge,
            Budget {
                input_tokens: spent,
                retry: true,
                ..budget()
            }
        )
        .await
        .is_err());
    assert_eq!(
        judge.calls, 2,
        "failed completion tokens cannot be reclaimed for a retry"
    );
    assert_eq!(
        fixture.ledger.assessment_status().unwrap(),
        status,
        "denied dispatch adds no charge"
    );
    fixture
        .assess(
            &mut judge,
            Budget {
                retry: true,
                ..budget()
            },
        )
        .await
        .unwrap();
    assert_eq!(judge.calls, 3);
}

#[tokio::test]
async fn purpose_without_proven_obligations_does_not_invent_work_or_recommend_release() {
    let mut fixture = Fixture::new();
    let mut judge = Judge {
        fault: Some("purpose-only"),
        satisfied: true,
        ..Default::default()
    };
    fixture.assess(&mut judge, budget()).await.unwrap();
    let view = fixture.view();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(assessment["purpose"]["quote"], "Run the regression test.");
    assert_eq!(assessment["obligations"], json!([]));
    assert_eq!(assessment["release_recommended"], false);
    assert_eq!(view["items"][0]["blocked"], true);
    assert_eq!(judge.calls, 1, "no obligations means no completion request");
}

#[tokio::test]
async fn replay_receipts_preserve_input_and_responses_and_refuse_content_tampering() {
    let mut fixture = Fixture::new();
    fixture
        .assess(&mut Judge::default(), budget())
        .await
        .unwrap();
    let view = fixture.view();
    let packet = &view["items"][0]["entries"][0]["assessment"];
    assert!(packet.get("input").is_none());
    assert!(packet.get("responses").is_none());
    let id = packet["id"].as_str().unwrap().to_owned();
    let settings = cli::Settings {
        work: fixture.work.clone(),
        scope: "private:test".into(),
        host: "test".into(),
        root: fixture.root.clone(),
    };
    let mut receipt = cli::run(&settings, cli::Command::Receipt { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(
        receipt["input"]["statements"][0]["quote"],
        "Run the regression test."
    );
    assert_eq!(receipt["responses"].as_array().unwrap().len(), 2);
    assert_eq!(receipt["model"], JEV_MODEL_PINNED);
    receipt["release_recommended"] = json!(true);
    fixture
        .db()
        .execute(
            "UPDATE scratch_assessments SET body=?1 WHERE id=?2",
            rusqlite::params![receipt.to_string(), id],
        )
        .unwrap();
    let error = cli::run(&settings, cli::Command::Receipt { id })
        .await
        .unwrap_err();
    assert!(error.contains("digest mismatch"));
}

struct ChangeDuringInference {
    work: PathBuf,
    root: PathBuf,
    path: PathBuf,
    replace: bool,
    judge: Judge,
}

#[async_trait::async_trait]
impl Responder for ChangeDuringInference {
    async fn respond(
        &mut self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        if self.judge.calls == 0 {
            if self.replace {
                // Retain the old inode so replacement cannot accidentally reuse it.
                std::fs::rename(&self.path, self.root.join("old-work")).unwrap();
                std::fs::create_dir(&self.path).unwrap();
            } else {
                Ledger::open(&self.work, "private:test")?.annotate(
                    "test",
                    &self.root,
                    &self.path,
                    "needs-finalization",
                    "Review new evidence that arrived during inference",
                    None,
                )?;
            }
        }
        self.judge.respond(state, questions).await
    }
}

#[tokio::test]
async fn allocation_replacement_or_new_evidence_discards_the_inflight_receipt() {
    for replace in [false, true] {
        let mut fixture = Fixture::new();
        let mut judge = ChangeDuringInference {
            work: fixture.work.clone(),
            root: fixture.root.clone(),
            path: fixture.path.clone(),
            replace,
            judge: Judge {
                satisfied: true,
                ..Default::default()
            },
        };
        let result = fixture
            .ledger
            .assess(
                &mut judge,
                "test",
                &fixture.root,
                std::slice::from_ref(&fixture.path),
                budget(),
            )
            .await
            .unwrap();
        assert_eq!(result["assessed"], 0);
        let view = fixture.view();
        assert_eq!(view["items"][0]["blocked"], true);
        assert_eq!(view["items"][0]["entries"][0]["assessment"], Value::Null);
        let receipts: u32 = fixture
            .db()
            .query_row("SELECT count(*) FROM scratch_assessments", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            receipts, 0,
            "stale inference cannot publish a replay receipt"
        );
        if replace {
            assert_eq!(view["items"][0]["entries"][0]["identity_matches"], false);
        } else {
            assert_eq!(
                view["items"][0]["entries"][0]["annotations"][0]["reason"],
                "Review new evidence that arrived during inference"
            );
        }
    }
}

#[tokio::test]
async fn active_owners_defer_assessment_until_actual_settlement() {
    let mut fixture = Fixture::new();
    fixture.record(json!([
        {"kind":"begin","id":"running-begin","invocation":"running","session":"ses","message":"msg","tool":"shell","repo":"repo","command":"cargo test regression","cwd":fixture.path},
        {"kind":"observe","id":"running-observe","path":fixture.path,"identity":fixture.identity(),"present":true,"activity":true,"owners":["running"]}
    ]));
    let mut judge = Judge::default();
    assert_eq!(
        fixture.assess(&mut judge, budget()).await.unwrap()["assessed"],
        0
    );
    assert_eq!(judge.calls, 0);
    assert_eq!(fixture.view()["items"][0]["blocked"], true);
    fixture.record(json!([{"kind":"end","id":"running-end","invocation":"running","outcome":"exited","exit":0}]));
    assert_eq!(
        fixture.assess(&mut judge, budget()).await.unwrap()["assessed"],
        1
    );
    assert_eq!(judge.calls, 2);
    assert_eq!(
        fixture.view()["items"][0]["entries"][0]["assessment_state"],
        "current"
    );
}
