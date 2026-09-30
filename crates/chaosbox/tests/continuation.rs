//! Continuation preserves user intent, abstentions and native source identity.
use chaosbox::continuation::{Window, prepare, render};
use chaosbox_jev::SystemOneResponse;
use serde_json::json;

fn response(count: usize, choice: &str) -> SystemOneResponse {
    let criteria = [
        "instruction",
        "decision",
        "finding",
        "pending",
        "execution",
        "irrelevant",
        "uncertain",
    ];
    serde_json::from_value(json!({"model":chaosbox_jev::JEV_MODEL_PINNED,"usage":{"input_tokens":10,"output_tokens":10},
        "answers":(0..count).map(|i| (format!("record_{i}"),json!({"type":"choice","choice":choice,"confidence":0.99,
            "probabilities":criteria.iter().map(|c| ((*c).to_owned(),if *c==choice {1.0} else {0.0})).collect::<std::collections::BTreeMap<_,_>>()}))).collect::<std::collections::BTreeMap<_,_>>()})).unwrap()
}

#[test]
fn latest_user_and_uncertain_source_survive_model_selection() {
    let text = "{\"id\":\"u1\",\"type\":\"user\",\"text\":\"Keep PostgreSQL coverage.\"}\n{\"id\":\"a1\",\"type\":\"assistant\",\"text\":\"Deployment is complete.\"}\n{\"id\":\"u2\",\"type\":\"user\",\"text\":\"Do not delete the running service yet.\"}\n";
    let input = prepare(
        text,
        &Window {
            scope: "private:can",
            repo: "canix",
            source: "opencode",
            session: "s1",
            start: 1,
            limit: 2,
        },
    )
    .unwrap();
    let packet = render(&input, &response(2, "irrelevant"), 120_000).unwrap();
    assert_eq!(packet["latest_user"]["id"], "u2");
    assert_eq!(packet["records"][0]["source"]["id"], "u1");
    assert_eq!(packet["omitted"][0]["id"], "a1");
    let unresolved = render(&input, &response(2, "uncertain"), 120_000).unwrap();
    assert_eq!(unresolved["records"].as_array().unwrap().len(), 2);
    assert_eq!(unresolved["records"][1]["classification"], "unresolved");
    assert_eq!(unresolved["records"][1]["assistant_assertion"], true);
    assert!(render(&input, &response(2, "pending"), 512).is_err());
}

#[test]
fn split_tools_changed_identity_and_incomplete_answers_are_rejected() {
    let text = "{\"id\":\"u1\",\"type\":\"user\",\"text\":\"Inspect.\"}\n{\"id\":\"a1\",\"type\":\"assistant\",\"tool_calls\":[{\"id\":\"call1\"}]}\n{\"id\":\"t1\",\"type\":\"tool\",\"tool_call_id\":\"call1\",\"content\":\"Success\"}";
    let mut window = Window {
        scope: "private:can",
        repo: "canix",
        source: "opencode",
        session: "s1",
        start: 3,
        limit: 2,
    };
    assert!(prepare(text, &window).is_err());
    window.start = 2;
    let input = prepare(text, &window).unwrap();
    assert!(render(&input, &response(1, "pending"), 120_000).is_err());
    let mut substituted = response(2, "pending");
    substituted.model = "jev-latest".into();
    assert!(render(&input, &substituted, 120_000).is_err());
}

#[tokio::test]
async fn successful_receipts_replay_and_interrupted_attempts_keep_their_budget() {
    use std::os::unix::fs::PermissionsExt;
    use chaosbox::continuation::cli::{Command, run};
    let dir = tempfile::tempdir().unwrap();
    let text = "{\"id\":\"u1\",\"type\":\"user\",\"text\":\"Keep the cited source.\"}";
    let input = prepare(
        text,
        &Window {
            scope: "private:can",
            repo: "canix",
            source: "opencode",
            session: "s1",
            start: 1,
            limit: 1,
        },
    )
    .unwrap();
    let source = dir.path().join("source.jsonl");
    let artifact = dir.path().join("input.json");
    std::fs::write(&source, text).unwrap();
    std::fs::write(&artifact, serde_json::to_vec(&input).unwrap()).unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    let journal = rusqlite::Connection::open(work.join("continuation-receipts.sqlite")).unwrap();
    journal.execute_batch("CREATE TABLE identity(id TEXT PRIMARY KEY); CREATE TABLE attempts(key TEXT, attempt INTEGER, reserved INTEGER, status TEXT, response TEXT, PRIMARY KEY(key,attempt));").unwrap();
    journal
        .execute(
            "INSERT INTO identity VALUES (?1)",
            [serde_json::to_string(&(
                &input.scope,
                &input.repo,
                &input.source,
                &input.session,
                &input.snapshot,
            ))
            .unwrap()],
        )
        .unwrap();
    let key = chaosbox::continuation::key(&input).unwrap();
    journal
        .execute(
            "INSERT INTO attempts VALUES (?1,1,100,'reserved',NULL)",
            [&key],
        )
        .unwrap();
    let command = |retry, name: &str| Command::Assemble {
        input: artifact.clone(),
        source_jsonl: source.clone(),
        work: work.clone(),
        privacy_reviewed: true,
        max_requests: 1,
        max_input_tokens: 1_000_000,
        max_chars: 120_000,
        retry,
        output: dir.path().join(name),
    };
    assert!(run(command(false, "refused.json"))
        .await
        .unwrap_err()
        .contains("explicit --retry"));
    assert!(run(command(true, "exhausted.json"))
        .await
        .unwrap_err()
        .contains("budget exhausted"));
    journal
        .execute(
            "UPDATE attempts SET status='success',response=?1 WHERE key=?2",
            rusqlite::params![
                serde_json::to_string(&response(1, "uncertain")).unwrap(),
                key
            ],
        )
        .unwrap();
    run(command(false, "replayed.json")).await.unwrap();
    let packet: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("replayed.json")).unwrap()).unwrap();
    assert_eq!(packet["records"][0]["classification"], "unresolved");
    let count: i64 = journal
        .query_row("SELECT count(*) FROM attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    std::fs::write(
        &source,
        "{\"id\":\"u1\",\"type\":\"user\",\"text\":\"Changed source.\"}",
    )
    .unwrap();
    assert!(run(command(false, "changed.json"))
        .await
        .unwrap_err()
        .contains("changed from its pinned source"));
}
