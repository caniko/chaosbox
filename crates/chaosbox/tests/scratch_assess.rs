//! Semantic receipts must never authorize deletion or outrun their evidence.
use std::{collections::BTreeMap, path::Path};
use chaosbox::{
    scratch::{Ledger, Request, assessment::Budget},
    Responder,
};
use chaosbox_jev::{Question, SystemOneResponse, JEV_MODEL_PINNED};
use serde_json::{json, Value};

struct Judge {
    calls: usize,
    satisfied: bool,
    fail: bool,
}

struct ReleaseDuringInference {
    work: std::path::PathBuf,
    root: std::path::PathBuf,
    path: std::path::PathBuf,
    judge: Judge,
}

struct UncertainJudge(Judge);
#[async_trait::async_trait]
impl Responder for UncertainJudge {
    async fn respond(
        &mut self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        let mut response = self.0.respond(state, questions).await?;
        if let Some(chaosbox_jev::Answer::Choice(answer)) = response.answers.get_mut("role_0") {
            answer.confidence = 0.5;
        }
        Ok(response)
    }
}
#[async_trait::async_trait]
impl Responder for ReleaseDuringInference {
    async fn respond(
        &mut self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        if self.judge.calls == 0 {
            Ledger::open(&self.work, "private:can")?.annotate(
                "test",
                &self.root,
                &self.path,
                "released",
                "Work finalized while Jev was evaluating",
                None,
            )?;
        }
        self.judge.respond(state, questions).await
    }
}
#[async_trait::async_trait]
impl Responder for Judge {
    async fn respond(
        &mut self,
        _: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        self.calls += 1;
        if self.fail {
            return Err("offline".into());
        }
        let answers: BTreeMap<_,_> = questions.iter().map(|(id,q)| {
            let Question::Choice { criteria, .. } = q else { panic!("Choice expected") };
            let selected = if id == "purpose" { "s0" }
                else if id.starts_with("role_") { "obligation" }
                else if id.starts_with("status_") { if self.satisfied { "satisfied" } else { "outstanding" } }
                else if id.starts_with("support_") { "e0" }
                else { "high" };
            assert!(criteria.contains_key(selected), "{id}: {criteria:?}");
            let probabilities: BTreeMap<_,_> = criteria.keys().map(|k| (k.clone(),f64::from(k == selected))).collect();
            (id.clone(),json!({"type":"choice","choice":selected,"confidence":1.0,"probabilities":probabilities}))
        }).collect();
        Ok(serde_json::from_value(json!({"model":JEV_MODEL_PINNED,"usage":{"input_tokens":1,"output_tokens":1},"answers":answers})).unwrap())
    }
}

fn seed(ledger: &mut Ledger, root: &Path, path: &Path) {
    ledger.record(&serde_json::from_value::<Request>(json!({"version":1,"scope":"private:can","host":"test","root":root,"events":[
        {"kind":"begin","id":"begin","invocation":"call","session":"ses","message":"msg","tool":"shell","repo":"repo","command":"true","cwd":root,
         "sources":[{"record":{"id":"user","type":"user","text":"Integrate the saved patch and run the regression test."},"pointer":"/text"}]},
        {"kind":"observe","id":"observe","path":path,"identity":chaosbox::scratch::identity(path,root).unwrap(),"present":true,"activity":true,"owners":["call"]},
        {"kind":"end","id":"end","invocation":"call","outcome":"exited","exit":0},
        {"kind":"coverage","id":"live","observer":"test","healthy":true,"detail":"watching"}
    ]})).unwrap()).unwrap();
}
fn budget() -> Budget {
    Budget {
        requests: 20,
        input_tokens: 1_000_000,
        retry: false,
    }
}

#[tokio::test]
async fn holds_are_cited_cached_and_explicit_release_is_required() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let path = root.join("patch");
    std::fs::create_dir_all(&path).unwrap();
    let mut ledger = Ledger::open(&fixture.path().join("custody"), "private:can").unwrap();
    seed(&mut ledger, &root, &path);
    let mut judge = Judge {
        calls: 0,
        satisfied: false,
        fail: false,
    };
    ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            budget(),
        )
        .await
        .unwrap();
    let view = ledger
        .query("test", &root, std::slice::from_ref(&path), 100)
        .unwrap();
    let entry = &view["items"][0]["entries"][0];
    assert_eq!(entry["disposition"], "needs-finalization");
    assert_eq!(entry["assessment"]["fresh"], true);
    assert_eq!(
        entry["assessment"]["obligations"][0]["status"],
        "outstanding"
    );
    assert!(entry["assessment"]["purpose"]["hash"].is_string());
    assert_eq!(entry["assessment"]["release_recommended"], false);
    ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            budget(),
        )
        .await
        .unwrap();
    assert_eq!(
        judge.calls, 2,
        "unchanged evidence reuses both assessment stages"
    );
    ledger
        .annotate(
            "test",
            &root,
            &path,
            "released",
            "Patch reviewed and task abandoned",
            None,
        )
        .unwrap();
    let view = ledger
        .query("test", &root, std::slice::from_ref(&path), 100)
        .unwrap();
    assert_eq!(view["items"][0]["blocked"], false);
    assert_eq!(view["items"][0]["entries"][0]["assessment"]["fresh"], false);
}

#[tokio::test]
async fn failure_is_durable_and_successful_command_is_not_completion_proof() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let path = root.join("patch");
    std::fs::create_dir_all(&path).unwrap();
    let mut ledger = Ledger::open(&fixture.path().join("custody"), "private:can").unwrap();
    seed(&mut ledger, &root, &path);
    let mut judge = Judge {
        calls: 0,
        satisfied: true,
        fail: true,
    };
    assert!(ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            budget()
        )
        .await
        .is_err());
    judge.fail = false;
    assert!(ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            budget()
        )
        .await
        .is_err());
    assert_eq!(judge.calls, 1, "failure must not be retried silently");
    let mut retry = budget();
    retry.retry = true;
    ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            retry,
        )
        .await
        .unwrap();
    let view = ledger.query("test", &root, &[path], 100).unwrap();
    assert_eq!(view["items"][0]["blocked"], true);
    assert_eq!(
        view["items"][0]["entries"][0]["assessment"]["release_recommended"], false,
        "a generic successful command is not proof the patch was integrated"
    );
}

#[tokio::test]
async fn concurrent_release_invalidates_the_inflight_hold_without_blocking_capture() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let path = root.join("patch");
    let work = fixture.path().join("custody");
    std::fs::create_dir_all(&path).unwrap();
    let mut ledger = Ledger::open(&work, "private:can").unwrap();
    seed(&mut ledger, &root, &path);
    let mut judge = ReleaseDuringInference {
        work,
        root: root.clone(),
        path: path.clone(),
        judge: Judge {
            calls: 0,
            satisfied: false,
            fail: false,
        },
    };
    ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            budget(),
        )
        .await
        .unwrap();
    let view = ledger.query("test", &root, &[path], 100).unwrap();
    assert_eq!(view["items"][0]["blocked"], false);
    assert_eq!(view["items"][0]["entries"][0]["assessment"], Value::Null);
}

#[tokio::test]
async fn uncertain_classification_keeps_original_work_evidence_visible_and_never_recommends_release(
) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let path = root.join("patch");
    std::fs::create_dir_all(&path).unwrap();
    let mut ledger = Ledger::open(&fixture.path().join("custody"), "private:can").unwrap();
    seed(&mut ledger, &root, &path);
    let mut judge = UncertainJudge(Judge {
        calls: 0,
        satisfied: true,
        fail: false,
    });
    ledger
        .assess(
            &mut judge,
            "test",
            &root,
            std::slice::from_ref(&path),
            budget(),
        )
        .await
        .unwrap();
    let view = ledger.query("test", &root, &[path], 100).unwrap();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(
        assessment["unresolved_sources"][0]["quote"],
        "Integrate the saved patch and run the regression test."
    );
    assert_eq!(assessment["release_recommended"], false);
    assert_eq!(view["items"][0]["blocked"], true);
    assert_eq!(
        judge.0.calls, 1,
        "unresolved roles are not invented into obligations"
    );
}

#[tokio::test]
async fn budgets_survive_restart_and_link_assertions_never_become_verified_receipts() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let path = root.join("patch");
    let work = fixture.path().join("custody");
    std::fs::create_dir_all(&path).unwrap();
    let mut ledger = Ledger::open(&work, "private:can").unwrap();
    seed(&mut ledger, &root, &path);
    let mut judge = Judge {
        calls: 0,
        satisfied: false,
        fail: false,
    };
    let mut cap = budget();
    cap.requests = 1;
    assert!(ledger
        .assess(&mut judge, "test", &root, std::slice::from_ref(&path), cap)
        .await
        .is_err());
    assert_eq!(judge.calls, 1);
    drop(ledger);
    let mut ledger = Ledger::open(&work, "private:can").unwrap();
    cap.retry = true;
    assert!(ledger
        .assess(&mut judge, "test", &root, std::slice::from_ref(&path), cap)
        .await
        .is_err());
    assert_eq!(judge.calls, 1);
    ledger.record(&serde_json::from_value::<Request>(json!({"version":1,"scope":"private:can","host":"test","root":root,"events":[
        {"kind":"link","id":"commit","path":path,"identity":chaosbox::scratch::identity(&path,&root).unwrap(),"category":"commit","reference":"commit:abc","description":"Claims integration"}
    ]})).unwrap()).unwrap();
    let view = ledger.query("test", &root, &[path], 100).unwrap();
    assert_eq!(
        view["items"][0]["entries"][0]["links"][0]["verified"],
        false
    );
    assert_eq!(
        view["items"][0]["entries"][0]["assessment_state"],
        "pending"
    );
}

#[tokio::test]
#[ignore = "explicit two-request Jev pilot using synthetic source evidence"]
async fn live_jev_keeps_an_unintegrated_patch_held_after_an_unrelated_success() {
    let fixture = tempfile::tempdir_in("/data/scratch/tmp/opencode").unwrap();
    let root = fixture.path().join("scratch");
    let path = root.join("patch");
    std::fs::create_dir_all(&path).unwrap();
    let mut ledger = Ledger::open(&fixture.path().join("custody"), "private:can").unwrap();
    seed(&mut ledger, &root, &path); // fabricated user text and a recorded `true` exit
    let client = chaosbox_jev::JevClient::new(chaosbox_jev::JevPolicy {
        max_requests: 2,
        max_retries: 0,
        max_input_tokens: 100_000,
        ..Default::default()
    })
    .unwrap();
    let mut responder = chaosbox::LiveResponder::new(client);
    ledger
        .assess(
            &mut responder,
            "test",
            &root,
            std::slice::from_ref(&path),
            Budget {
                requests: 2,
                input_tokens: 100_000,
                retry: false,
            },
        )
        .await
        .unwrap();
    let view = ledger.query("test", &root, &[path], 100).unwrap();
    let assessment = &view["items"][0]["entries"][0]["assessment"];
    assert_eq!(assessment["model"], JEV_MODEL_PINNED);
    assert_eq!(assessment["fresh"], true);
    assert!(!assessment["obligations"].as_array().unwrap().is_empty());
    assert_eq!(assessment["release_recommended"], false);
    assert_eq!(view["items"][0]["blocked"], true);
    assert!(responder.usage().0 <= 2);
}
