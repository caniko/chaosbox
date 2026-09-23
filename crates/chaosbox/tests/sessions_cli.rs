//! `chaosbox sessions` exercised through the real executable.
//!
//! `tests/sessions.rs` calls `cli::run` with an already-built `Command`,
//! which skips the two things that actually decide an operator's exit code:
//! whether clap accepts the arguments they typed, and whether `main` turns a
//! failure into a non-zero status instead of a successful process that
//! merely printed an error. Both are proved here by running the binary.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use chaosbox::sessions::digest::session_digest;
use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::TempDir;

/// Session id the fixture destination holds by default.
const SESSION: &str = "ses_fixture01";
/// Second session, added by the pass that needs to bound itself.
const OTHER: &str = "ses_fixture02";

/// Create a staging root with an empty inventory and a one-session
/// destination database.
fn fixture() -> (TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path().join("staging");
    fs::create_dir_all(root.join("journal-v2")).expect("journal");
    let connection = Connection::open(root.join("destination.db")).expect("destination");
    connection
        .execute_batch(
            "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time INTEGER, cost REAL);
             CREATE TABLE session_message (session_id TEXT, seq INTEGER, id TEXT, role TEXT);
             CREATE TABLE project_directory (root TEXT);
             CREATE TABLE worktree (root TEXT);
             INSERT INTO session_v2 VALUES
                ('ses_fixture01', 1790166653727, 3.6143699999999996);
             INSERT INTO session_message VALUES
                ('ses_fixture01', 1, 'msg_1', 'user');",
        )
        .expect("schema");
    drop(connection);
    declare(&root, &[]);
    (directory, root)
}

/// Add a second session, so a pass has something to bound itself across.
fn add_session(root: &Path, id: &str) {
    let connection = Connection::open(root.join("destination.db")).expect("destination");
    connection
        .execute(
            "INSERT INTO session_v2 VALUES (?1, 1790166653727, 0.448854)",
            [id],
        )
        .expect("session");
    connection
        .execute(
            "INSERT INTO session_message VALUES (?1, 1, 'msg_' || ?1, 'user')",
            [id],
        )
        .expect("message");
}

/// The destination digest of `session`, as a receipt would record it.
fn digest_for(root: &Path, session: &str) -> String {
    let connection = Connection::open_with_flags(
        root.join("destination.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("destination");
    session_digest(&connection, session)
        .expect("fixture digests")
        .digest
}

/// A base receipt for `session`, attesting to `digest`.
fn receipt(session: &str, digest: &str) -> Value {
    json!({
        "sessionID": session,
        "source": "primary",
        "destinationDigest": digest,
        "inputDigest": digest,
        "recoveryDigest": digest,
        "messages": 1,
        "transformation": "interrupted-draft-v1",
        "drafts": 0,
    })
}

/// Write one receipt file into the journal.
fn write_receipt(root: &Path, session: &str, digest: &str) {
    fs::create_dir_all(root.join("journal-v2")).expect("journal");
    fs::write(
        root.join(format!("journal-v2/{session}.json")),
        serde_json::to_string(&receipt(session, digest)).expect("receipt serializes"),
    )
    .expect("receipt");
}

/// Record the sessions the driver claims it migrated.
fn declare(root: &Path, ids: &[&str]) {
    let verified: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, "source": "primary", "messages": 1 }))
        .collect();
    let progress = json!({
        "total": ids.len(),
        "verified": verified,
        "deferred": [],
        "errors": [],
        "complete": true,
        "readyForCutover": false,
        "driverDigest": "62f6fb3544875b4f22c63d3df315e2c7d5106eff21bedc26c7e1fd696b9f6604",
    });
    fs::write(
        root.join("progress.json"),
        serde_json::to_string(&progress).expect("progress serializes"),
    )
    .expect("progress");
}

/// Run the real binary and capture everything an operator would see.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(args)
        // The ambient environment must not decide whether this campaign
        // resolves, or the test would pass in one shell and fail in another.
        .env_remove("CHAOSBOX_SESSION_CAMPAIGN")
        .output()
        .expect("chaosbox runs")
}

/// The JSON report the binary printed on stdout.
fn report(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("stdout should be a report: {error}\n{stdout}"))
}

/// A complete, clean pass exits `0` and says which inventory it verified.
#[test]
fn a_complete_clean_pass_exits_zero() {
    let (held, root) = fixture();
    write_receipt(&root, SESSION, &digest_for(&root, SESSION));
    declare(&root, &[SESSION]);

    let root_arg = root.to_string_lossy().into_owned();
    let output = run(&["sessions", "verify", "--root", &root_arg]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(&output);
    assert_eq!(report["sessions_verified"], 1);
    assert_eq!(report["complete"], true);
    assert_eq!(report["inventory"]["reconciled"], true);
    assert_eq!(report["inventory"]["expected"], 1);
    assert_eq!(report["inventory"]["destination_rows"], 1);
    assert_eq!(report["snapshot"]["consistent_read"], true);
    assert_eq!(report["checked_sources"], false);
    drop(held);
}

/// Bounding the pass exits `1` until `--allow-partial` says the operator
/// accepts a pass that cannot vouch for what it skipped.
#[test]
fn a_bounded_pass_exits_one_until_partial_is_allowed() {
    let (held, root) = fixture();
    add_session(&root, OTHER);
    write_receipt(&root, SESSION, &digest_for(&root, SESSION));
    write_receipt(&root, OTHER, &digest_for(&root, OTHER));
    declare(&root, &[SESSION, OTHER]);

    let root_arg = root.to_string_lossy().into_owned();
    let bounded = run(&["sessions", "verify", "--root", &root_arg, "--limit", "1"]);
    assert_eq!(
        bounded.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&bounded.stderr)
    );

    let allowed = run(&[
        "sessions",
        "verify",
        "--root",
        &root_arg,
        "--limit",
        "1",
        "--allow-partial",
    ]);
    assert_eq!(
        allowed.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    assert_eq!(report(&allowed)["complete"], false);
    drop(held);
}

/// An inventory the journals do not reconcile against exits `1` even with
/// `--allow-partial`: accepting a bounded pass is not accepting a vacuous
/// one.
#[test]
fn an_unreconciled_inventory_exits_one() {
    let (held, root) = fixture();
    declare(&root, &[SESSION]);

    let root_arg = root.to_string_lossy().into_owned();
    let output = run(&["sessions", "verify", "--root", &root_arg, "--allow-partial"]);

    assert_eq!(output.status.code(), Some(1));
    let report = report(&output);
    assert_eq!(report["inventory"]["reconciled"], false);
    assert_eq!(report["inventory"]["missing_receipts"][0], SESSION);
    drop(held);
}

/// A failed verification prints a report and exits `1`; a campaign that
/// cannot be read prints an error and exits `1`. Both are non-zero, but only
/// one produced a report, and an operator has to tell them apart.
#[test]
fn a_failure_and_a_cannot_run_both_exit_non_zero() {
    let (held, root) = fixture();
    write_receipt(&root, SESSION, &"0".repeat(64));
    declare(&root, &[SESSION]);

    let root_arg = root.to_string_lossy().into_owned();
    let failed = run(&["sessions", "verify", "--root", &root_arg]);
    assert_eq!(
        failed.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        report(&failed)["digest_mismatch"].as_array().is_some(),
        "a failed pass still produced a report"
    );

    // A directory that is not a staging root: no report, just an error.
    let empty = tempfile::tempdir().expect("temp directory");
    let empty_arg = empty.path().to_string_lossy().into_owned();
    let broken = run(&["sessions", "verify", "--root", &empty_arg]);
    assert_eq!(broken.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&broken.stderr);
    assert!(
        stderr.contains("sessions:"),
        "the error names its subcommand: {stderr}"
    );
    assert!(
        broken.stdout.is_empty(),
        "no report was produced, so nothing claims to have verified: {}",
        String::from_utf8_lossy(&broken.stdout)
    );
    drop(held);
}

/// Naming a session the campaign has no receipt for exits `1` with an
/// explanation rather than running a pass over nothing.
#[test]
fn an_unknown_session_exits_one_with_an_explanation() {
    let (held, root) = fixture();
    write_receipt(&root, SESSION, &digest_for(&root, SESSION));
    declare(&root, &[SESSION]);

    let root_arg = root.to_string_lossy().into_owned();
    let output = run(&[
        "sessions",
        "verify",
        "--root",
        &root_arg,
        "--session",
        "ses_notpresent",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ses_notpresent"),
        "the error names the session: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "an error is not a report: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    drop(held);
}

/// With no `--root` and no environment variable there is nothing to verify,
/// and the process must not exit `0` for having done nothing.
#[test]
fn a_missing_campaign_root_exits_one() {
    let output = run(&["sessions", "verify"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no campaign root"),
        "unexpected stderr: {stderr}"
    );
}

/// Arguments the parser does not recognise are rejected by the parser, which
/// is a different exit code from a verification that ran and failed. An
/// operator typo must never look like a bad campaign.
#[test]
fn an_unrecognised_flag_is_rejected_by_the_parser() {
    let output = run(&["sessions", "verify", "--definitely-not-a-flag"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "a usage error produces no report");
}

/// `status` reports and exits `0`, printing a JSON document on stdout.
#[test]
fn status_prints_a_report_and_exits_zero() {
    let (held, root) = fixture();
    write_receipt(&root, SESSION, &digest_for(&root, SESSION));
    declare(&root, &[SESSION]);

    let root_arg = root.to_string_lossy().into_owned();
    let output = run(&["sessions", "status", "--root", &root_arg]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(&output);
    assert_eq!(report["receipts"]["sessions"], 1);
    assert_eq!(report["destination"]["integrity"]["quickCheck"], "ok");
    assert_eq!(
        report["readyForCutover"], false,
        "verification never turns the cutover flag on"
    );
    drop(held);
}
