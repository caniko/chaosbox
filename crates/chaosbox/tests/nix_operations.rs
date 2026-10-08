//! Real isolated-store addition, retry, intelligence and cleanup contracts.
use std::{fs, path::Path};
use chaosbox::nix::{AddMode, Invocation, Ledger, Settings};

#[tokio::test]
async fn rejected_execution_write_settles_the_native_receipt_without_reexecution() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("input"),
        "survive first receipt write failure",
    )
    .unwrap();
    let mut config = settings(temp.path());
    drop(Ledger::open(&config).unwrap());
    let database = rusqlite::Connection::open(config.work.join("nix.sqlite")).unwrap();
    database.execute_batch("CREATE TRIGGER fail_execution BEFORE INSERT ON executions BEGIN SELECT RAISE(ABORT, 'fixture execution write failure'); END;").unwrap();
    let request = invocation(
        temp.path(),
        "retain execution evidence",
        "execution-write-failure",
    );
    let receipt = chaosbox::nix::add(&config, request.clone()).await.unwrap();
    assert_eq!(receipt["outcome"], "succeeded");
    assert!(receipt["objects"][0]["path"].is_string());
    assert!(receipt["journal_warning"]
        .as_str()
        .unwrap()
        .contains("fixture execution"));
    let evidence = Ledger::read(&config)
        .unwrap()
        .evidence("fixture", &request.id)
        .unwrap();
    assert_eq!(evidence["settled"], true);
    assert_eq!(evidence["receipt"], receipt);
    fs::remove_file(&request.path).unwrap();
    config.nix = "/no-such-executable".into();
    assert_eq!(chaosbox::nix::add(&config, request).await.unwrap(), receipt);
}

#[tokio::test]
async fn total_receipt_write_failure_reports_observed_evidence_and_blocks_reexecution() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("input"),
        "retain evidence when both tables reject writes",
    )
    .unwrap();
    let config = settings(temp.path());
    drop(Ledger::open(&config).unwrap());
    let database = rusqlite::Connection::open(config.work.join("nix.sqlite")).unwrap();
    database.execute_batch("CREATE TRIGGER fail_execution BEFORE INSERT ON executions BEGIN SELECT RAISE(ABORT, 'fixture execution write failure'); END; CREATE TRIGGER fail_settlement BEFORE INSERT ON settlements BEGIN SELECT RAISE(ABORT, 'fixture settlement write failure'); END;").unwrap();
    let request = invocation(
        temp.path(),
        "report unpersisted outcome",
        "all-writes-failed",
    );
    let error = chaosbox::nix::add(&config, request.clone())
        .await
        .unwrap_err();
    let receipt: serde_json::Value =
        serde_json::from_str(error.split_once("observed_receipt=").unwrap().1).unwrap();
    assert_eq!(receipt["outcome"], "succeeded");
    assert!(receipt["objects"][0]["path"]
        .as_str()
        .unwrap()
        .starts_with("/nix/store/"));
    assert!(chaosbox::nix::add(&config, request)
        .await
        .unwrap_err()
        .contains("operation unresolved"));
}

fn settings(root: &Path) -> Settings {
    Settings {
        work: root.join("ledger"),
        scope: "private:test".into(),
        host: "fixture".into(),
        store: format!("local?root={}", root.join("store").display()),
        nix: std::env::var("CHAOSBOX_TEST_NIX")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/nix".into())
            .into(),
        timeout_seconds: 30,
    }
}

fn invocation(root: &Path, reason: &str, id: &str) -> Invocation {
    Invocation {
        id: id.into(),
        repo: "fixture".into(),
        reason: reason.into(),
        cwd: root.into(),
        session: Some("ses_fixture".into()),
        message: Some("msg_fixture".into()),
        tool_call: Some(id.into()),
        path: root.join("input"),
        mode: AddMode::Nar,
    }
}

#[tokio::test]
async fn blank_reason_and_failed_journal_never_launch_nix() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "test").unwrap();
    let mut config = settings(temp.path());
    config.nix = "/no-such-executable".into();
    let error = chaosbox::nix::add(&config, invocation(temp.path(), " \n\t", "blank"))
        .await
        .unwrap_err();
    assert!(error.contains("reason"));
    assert!(!config.work.exists());
    fs::write(&config.work, "not a directory").unwrap();
    assert!(
        chaosbox::nix::add(&config, invocation(temp.path(), "needed", "blocked"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn real_addition_has_durable_offline_evidence_and_exact_retry() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "provenance fixture").unwrap();
    let config = settings(temp.path());
    let request = invocation(temp.path(), "qualify a source candidate", "first");
    let receipt = chaosbox::nix::add(&config, request.clone()).await.unwrap();
    assert_eq!(receipt["outcome"], "succeeded", "{receipt}");
    assert!(receipt["objects"][0]["narHash"]
        .as_str()
        .unwrap()
        .starts_with("sha256-"));
    let path = receipt["objects"][0]["path"].as_str().unwrap();
    let ledger = Ledger::read(&config).unwrap();
    let packet = ledger.context("fixture", "source candidate", 5).unwrap();
    assert_eq!(packet["records"][0]["reason"], request.reason);
    assert_eq!(
        ledger.evidence("fixture", "first").unwrap()["receipt"],
        receipt
    );
    assert_eq!(
        ledger.context("fixture", path, 5).unwrap()["records"][0]["id"],
        "first"
    );
    assert!(ledger.evidence("foreign-repo", "first").is_err());
    assert_eq!(
        ledger.query(Some(path), 0, 20).unwrap()["items"][0]["retention"],
        "unrooted"
    );
    assert_eq!(
        ledger.query(Some(path), 0, 20).unwrap()["items"][0]["filesystem_presence"],
        "present"
    );
    drop(ledger);
    fs::write(temp.path().join("input"), "changed after original addition").unwrap();
    assert_eq!(
        chaosbox::nix::add(&config, request.clone()).await.unwrap(),
        receipt
    );
    let mut conflict = request;
    conflict.reason = "different intent".into();
    assert!(chaosbox::nix::add(&config, conflict)
        .await
        .unwrap_err()
        .contains("identity"));
}

#[tokio::test]
async fn failed_final_settlement_preserves_execution_and_retry_reconciles_without_running_nix() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "survive failed final receipt").unwrap();
    let mut config = settings(temp.path());
    drop(Ledger::open(&config).unwrap());
    let database = rusqlite::Connection::open(config.work.join("nix.sqlite")).unwrap();
    database.execute_batch("CREATE TRIGGER fail_settlement BEFORE INSERT ON settlements BEGIN SELECT RAISE(ABORT, 'fixture receipt write failure'); END;").unwrap();
    let request = invocation(
        temp.path(),
        "recover actual execution",
        "settlement-failure",
    );
    assert!(chaosbox::nix::add(&config, request.clone())
        .await
        .unwrap_err()
        .contains("fixture receipt"));
    let evidence = Ledger::read(&config)
        .unwrap()
        .evidence("fixture", &request.id)
        .unwrap();
    assert_eq!(evidence["settled"], false);
    assert_eq!(evidence["receipt"]["outcome"], "succeeded");
    assert!(evidence["receipt"]["objects"][0]["path"].is_string());
    database
        .execute_batch("DROP TRIGGER fail_settlement;")
        .unwrap();
    fs::remove_file(&request.path).unwrap();
    config.nix = "/no-such-executable".into();
    let replay = chaosbox::nix::add(&config, request.clone()).await.unwrap();
    assert_eq!(replay, evidence["receipt"]);
    assert_eq!(
        Ledger::read(&config)
            .unwrap()
            .evidence("fixture", &request.id)
            .unwrap()["settled"],
        true
    );
}

#[test]
fn fresh_cli_process_recovers_a_prior_sessions_reason_and_native_evidence() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "fresh-session fixture").unwrap();
    let config = settings(temp.path());
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
            .args(args)
            .current_dir(temp.path())
            .env("CHAOSBOX_NIX_BIN", &config.nix)
            .env("CHAOSBOX_NIX_WORK", &config.work)
            .env("CHAOSBOX_NIX_HOST", &config.host)
            .env("CHAOSBOX_NIX_STORE", &config.store)
            .env("CHAOSBOX_NIX_SCOPE", &config.scope)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let receipt = run(&[
        "nix",
        "add",
        "--repo",
        "fixture",
        "--reason",
        "compare an exact upstream candidate",
        "--id",
        "session-one-add",
        "--session",
        "ses_one",
        "--",
        "./input",
    ]);
    let path = receipt["objects"][0]["path"].as_str().unwrap();
    let packet = run(&["nix", "context", "--repo", "fixture", path]);
    assert_eq!(
        packet["records"][0]["reason"],
        "compare an exact upstream candidate"
    );
    let evidence = run(&["nix", "evidence", "--repo", "fixture", "session-one-add"]);
    assert_eq!(evidence["request"]["session"], "ses_one");
    assert_eq!(evidence["receipt"], receipt);
    assert_eq!(
        run(&["nix", "query", "--path", path])["items"][0]["filesystem_presence"],
        "present"
    );
}

#[tokio::test]
async fn retry_during_metadata_capture_replays_one_canonical_settlement() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "canonical concurrent receipt").unwrap();
    let mut config = settings(temp.path());
    let native = config.nix.clone();
    config.nix = temp.path().join("nix-wrapper");
    fs::write(&config.nix, format!("#!/bin/sh\nif [ \"$1\" = path-info ]; then\n  touch metadata-started\n  while [ ! -e release-metadata ]; do sleep 0.01; done\nelse\n  echo add >> launched\nfi\nexec '{}' \"$@\"\n", native.display())).unwrap();
    fs::set_permissions(&config.nix, fs::Permissions::from_mode(0o700)).unwrap();
    let request = invocation(
        temp.path(),
        "one canonical receipt under concurrent retry",
        "metadata-retry",
    );
    let first = tokio::spawn({
        let config = config.clone();
        let request = request.clone();
        async move { chaosbox::nix::add(&config, request).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !temp.path().join("metadata-started").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let replay = chaosbox::nix::add(&config, request.clone()).await.unwrap();
    assert_eq!(replay["outcome"], "succeeded");
    fs::write(temp.path().join("release-metadata"), "").unwrap();
    assert_eq!(first.await.unwrap().unwrap(), replay);
    assert_eq!(chaosbox::nix::add(&config, request).await.unwrap(), replay);
    assert_eq!(
        fs::read_to_string(temp.path().join("launched")).unwrap(),
        "add\n"
    );
}

#[tokio::test]
async fn flat_addition_preserves_literal_reason_and_real_content_metadata() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "exact bytes").unwrap();
    let config = settings(temp.path());
    let mut request = invocation(
        temp.path(),
        "literal ' reason; $(touch ./unwanted)\n--store ssh://remote",
        "flat",
    );
    request.mode = AddMode::Flat;
    let receipt = chaosbox::nix::add(&config, request.clone()).await.unwrap();
    assert_eq!(receipt["outcome"], "succeeded");
    assert_eq!(receipt["reason"], request.reason);
    assert!(receipt["objects"][0]["narHash"].is_string());
    assert!(!temp.path().join("unwanted").exists());
}

#[tokio::test]
async fn missing_binary_is_not_started_and_missing_input_is_failed() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = settings(temp.path());
    let failed = chaosbox::nix::add(
        &config,
        invocation(temp.path(), "missing input", "input-missing"),
    )
    .await
    .unwrap();
    assert_eq!(failed["outcome"], "failed");
    config.nix = "/no-such-executable".into();
    let failed = chaosbox::nix::add(
        &config,
        invocation(temp.path(), "missing binary", "binary-missing"),
    )
    .await
    .unwrap();
    assert_eq!(failed["outcome"], "not-started");
    assert_eq!(failed["objects"], serde_json::json!([]));
}

#[tokio::test]
async fn shared_objects_keep_independent_reasons_and_survive_collection_as_history() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "shared fixture").unwrap();
    let config = settings(temp.path());
    let first = chaosbox::nix::add(&config, invocation(temp.path(), "first consumer", "one"))
        .await
        .unwrap();
    let second = chaosbox::nix::add(&config, invocation(temp.path(), "second consumer", "two"))
        .await
        .unwrap();
    assert_eq!(first["objects"][0]["path"], second["objects"][0]["path"]);
    let status = std::process::Command::new(&config.nix)
        .args([
            "store",
            "gc",
            "--store",
            &config.store,
            "--extra-experimental-features",
            "nix-command",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let path = first["objects"][0]["path"].as_str().unwrap();
    let query = Ledger::read(&config)
        .unwrap()
        .query(Some(path), 0, 20)
        .unwrap();
    assert_eq!(query["items"].as_array().unwrap().len(), 2);
    assert_eq!(query["items"][0]["filesystem_presence"], "absent");
    assert_eq!(query["items"][1]["reason"], "second consumer");
}

#[test]
fn pending_intent_cannot_be_reexecuted_or_cross_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let config = settings(temp.path());
    let request = invocation(temp.path(), "crashed writer", "pending");
    let mut ledger = Ledger::open(&config).unwrap();
    assert!(ledger.begin(&request).unwrap().is_none());
    assert!(ledger.begin(&request).unwrap_err().contains("unresolved"));
    drop(ledger);
    let mut other = config;
    other.host = "other-host".into();
    assert!(Ledger::read(&other).is_err());
}

#[test]
fn cli_rejects_missing_reason_and_execution_setting_overrides() {
    let temp = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_chaosbox");
    for args in [
        vec!["nix", "add", "--repo", "fixture", "--", "./input"],
        vec![
            "nix", "add", "--repo", "fixture", "--reason", "needed", "--nix", "/bin/sh", "--",
            "./input",
        ],
    ] {
        let output = std::process::Command::new(binary)
            .args(args)
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn concurrent_duplicate_has_one_durable_intent_and_never_restarts() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("input"), "concurrent source").unwrap();
    let config = settings(temp.path());
    let request = invocation(temp.path(), "one intent", "same-call");
    let (one, two) = tokio::join!(
        chaosbox::nix::add(&config, request.clone()),
        chaosbox::nix::add(&config, request)
    );
    assert!(one.is_ok() || two.is_ok());
    let query = Ledger::read(&config).unwrap().query(None, 0, 20).unwrap();
    assert_eq!(query["items"].as_array().unwrap().len(), 1);
    assert_eq!(query["items"][0]["outcome"], "succeeded");
}

#[tokio::test]
async fn timed_out_or_cancelled_runner_retains_unknown_execution_for_inspection() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let mut config = settings(temp.path());
    let script = temp.path().join("slow-nix");
    // A deterministic process fixture exercises runner ownership without a large Nix import.
    let sleep = std::env::var("PATH")
        .unwrap()
        .split(':')
        .map(|p| Path::new(p).join("sleep"))
        .find(|p| p.is_file())
        .unwrap();
    let shell = std::env::var("PATH")
        .unwrap()
        .split(':')
        .map(|p| Path::new(p).join("sh"))
        .find(|p| p.is_file())
        .unwrap();
    fs::write(
        &script,
        format!("#!{}\nexec {} 30\n", shell.display(), sleep.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    config.nix = script;
    config.timeout_seconds = 1;
    let receipt = chaosbox::nix::add(
        &config,
        invocation(temp.path(), "bounded process", "timeout"),
    )
    .await
    .unwrap();
    assert_eq!(receipt["outcome"], "unresolved");
    assert!(receipt["error"].as_str().unwrap().contains("timed out"));
    let request = invocation(temp.path(), "interrupted caller", "cancelled");
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(50),
        chaosbox::nix::add(&config, request.clone())
    )
    .await
    .is_err());
    let packet = Ledger::read(&config)
        .unwrap()
        .evidence("fixture", "cancelled")
        .unwrap();
    assert!(packet["receipt"].is_null());
    assert!(chaosbox::nix::add(&config, request)
        .await
        .unwrap_err()
        .contains("unresolved"));
}
