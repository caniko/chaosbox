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

/// `adopt` pins a store into the campaign and exits `0`, printing the
/// record it wrote.
#[test]
fn adopt_registers_a_store_and_exits_zero() {
    let (held, root) = fixture();
    let campaign = root.parent().expect("parent").to_path_buf();
    let campaign_arg = campaign.to_string_lossy().into_owned();
    let db_arg = root.join("destination.db").to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        &db_arg,
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record = report(&output);
    assert_eq!(record["name"], "staging");
    assert_eq!(record["dbPath"], db_arg);
    assert_eq!(record["schema"]["marker"], "v2");
    assert_eq!(record["counts"]["sessions"], 1);
    assert_eq!(report(&output)["counts"]["messages"], 1);
    assert_eq!(record["health"]["quickCheck"], "ok");
    assert_eq!(record["holders"]["clear"], true);
    assert_eq!(record["held"], false);
    assert_eq!(record["boundaryRecord"], serde_json::Value::Null);
    let stored: Value = serde_json::from_str(
        &fs::read_to_string(campaign.join("adoption/staging.json")).expect("record on disk"),
    )
    .expect("record parses");
    assert_eq!(stored, record, "stdout and the record file agree");
    drop(held);
}

/// A store with writers attached is refused without `--allow-held`, and
/// admitted with `held: true` when the flag is passed.
#[test]
fn adopt_refuses_a_held_store_without_the_flag() {
    let (held, root) = fixture();
    let campaign = root.parent().expect("parent").to_path_buf();
    let campaign_arg = campaign.to_string_lossy().into_owned();
    let db_path = root.join("destination.db");
    let db_arg = db_path.to_string_lossy().into_owned();
    // This connection holds the database file open for the whole test.
    let _writer = Connection::open(&db_path).expect("holder");

    let refused = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        &db_arg,
    ]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        !campaign.join("adoption/staging.json").exists(),
        "a refused adoption writes no record"
    );

    let admitted = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        &db_arg,
        "--allow-held",
    ]);
    assert_eq!(
        admitted.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&admitted.stderr)
    );
    let record = report(&admitted);
    assert_eq!(record["held"], true);
    assert_eq!(record["holders"]["clear"], false);
    drop(held);
}

/// A relative `--db` cannot be pinned, so it is refused rather than
/// recorded against whatever directory the operator happened to run from.
#[test]
fn adopt_rejects_a_relative_db_path() {
    let (held, _root) = fixture();
    let campaign = tempfile::tempdir().expect("temp directory");
    let campaign_arg = campaign.path().to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        "relative/opencode.db",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("absolute"), "unexpected stderr: {stderr}");
    drop(held);
}

/// A name outside the stable key set is a usage error, not a refusal.
#[test]
fn an_unknown_adopt_name_is_rejected_by_the_parser() {
    let output = run(&[
        "sessions",
        "adopt",
        "--root",
        "/nowhere",
        "--name",
        "archive",
        "--db",
        "/nowhere/opencode.db",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

/// `install --dry-run` prints the pinned installer and its arguments and
/// executes nothing: the marker the script would write stays absent.
#[test]
fn install_dry_run_prints_the_plan_without_executing() {
    let campaign = tempfile::tempdir().expect("temp directory");
    let tools = campaign.path().join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let marker = campaign.path().join("executed.marker");
    let script = tools.join("install.mjs");
    fs::write(
        &script,
        format!(
            "import {{ writeFileSync }} from 'node:fs';\nwriteFileSync({:?}, 'ran');\n",
            marker.to_string_lossy().into_owned()
        ),
    )
    .expect("script");
    let digest = sha256_file(&script);
    fs::write(
        campaign.path().join("tools.json"),
        serde_json::to_string(&json!({
            "tools": { "install.mjs": {
                "path": script.to_string_lossy(),
                "sha256": digest,
            } },
        }))
        .expect("pins serialize"),
    )
    .expect("pins");
    let campaign_arg = campaign.path().to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "install",
        "--root",
        &campaign_arg,
        "--dir",
        "/target",
        "--source",
        "/staged.db",
        "--state",
        "/state.json",
        "--expect-sessions",
        "7913",
        "--dry-run",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan = report(&output);
    assert_eq!(plan["installer"], script.to_string_lossy().into_owned());
    assert!(!marker.exists(), "dry-run must not execute the installer");
}

/// A tool whose bytes do not match the pin is refused before anything runs.
#[test]
fn install_refuses_a_mismatched_tool_digest() {
    let campaign = tempfile::tempdir().expect("temp directory");
    let tools = campaign.path().join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let script = tools.join("install.mjs");
    fs::write(&script, "console.log('tampered');\n").expect("script");
    fs::write(
        campaign.path().join("tools.json"),
        serde_json::to_string(&json!({
            "tools": { "install.mjs": {
                "path": script.to_string_lossy(),
                "sha256": "0".repeat(64),
            } },
        }))
        .expect("pins serialize"),
    )
    .expect("pins");
    let campaign_arg = campaign.path().to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "install",
        "--root",
        &campaign_arg,
        "--dir",
        "/target",
        "--source",
        "/staged.db",
        "--state",
        "/state.json",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to run"),
        "unexpected stderr: {stderr}"
    );
}

/// `rollback` forwards to the pinned script and its exit code: the script
/// here echoes its arguments and succeeds.
#[test]
fn rollback_forwards_to_the_pinned_script() {
    let campaign = tempfile::tempdir().expect("temp directory");
    let tools = campaign.path().join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let script = tools.join("rollback.mjs");
    fs::write(
        &script,
        "console.log(JSON.stringify({ argv: process.argv.slice(2), ok: true }));\n",
    )
    .expect("script");
    let digest = sha256_file(&script);
    fs::write(
        campaign.path().join("tools.json"),
        serde_json::to_string(&json!({
            "tools": { "rollback.mjs": {
                "path": script.to_string_lossy(),
                "sha256": digest,
            } },
        }))
        .expect("pins serialize"),
    )
    .expect("pins");
    let campaign_arg = campaign.path().to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "rollback",
        "--root",
        &campaign_arg,
        "--state",
        "/state.json",
        "--record",
        "/record.json",
        "--config-live",
        "/live.json",
        "--config-baseline",
        "/baseline.json",
        "--config-restore",
        "true",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let echoed = report(&output);
    assert_eq!(echoed["ok"], true);
    let argv = echoed["argv"].as_array().expect("argv echoed");
    assert!(argv.contains(&json!("--config-restore")));
    assert!(argv.contains(&json!("true")));
}

/// A missing `--config-*` argument is a usage error, checked before any
/// freeze could stop a writer.
#[test]
fn rollback_without_its_config_arguments_is_a_usage_error() {
    let output = run(&[
        "sessions",
        "rollback",
        "--root",
        "/nowhere",
        "--state",
        "/state.json",
        "--record",
        "/record.json",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

/// SHA-256 over a file, the shape a tool pin carries.
fn sha256_file(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path).expect("script readable");
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer).expect("script reads");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("{:x}", hasher.finalize())
}

/// A database with no session tables is not a v2 store: adoption refuses it
/// rather than certifying an empty schema as healthy.
#[test]
fn adopt_refuses_a_store_without_session_tables() {
    let (held, root) = fixture();
    let campaign = root.parent().expect("parent").to_path_buf();
    let campaign_arg = campaign.to_string_lossy().into_owned();
    let empty = campaign.join("empty.db");
    let connection = Connection::open(&empty).expect("empty");
    connection
        .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
        .expect("schema");
    drop(connection);
    let db_arg = empty.to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        &db_arg,
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        !campaign.join("adoption/staging.json").exists(),
        "a refused adoption writes no record"
    );
    drop(held);
}

// ---- adoption + tool-closure hardening (cutover readiness) ----

/// A store with a WAL sidecar is not frozen: adoption refuses it rather than
/// hashing the main file alone and describing a torn view.
#[test]
fn adopt_refuses_a_store_with_a_wal_sidecar() {
    let (held, root) = fixture();
    let campaign = root.parent().expect("parent").to_path_buf();
    let campaign_arg = campaign.to_string_lossy().into_owned();
    let db_arg = root.join("destination.db").to_string_lossy().into_owned();
    fs::write(root.join("destination.db-wal"), b"uncheckpointed").expect("sidecar");

    let output = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        &db_arg,
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        !campaign.join("adoption/staging.json").exists(),
        "a refused adoption writes no record"
    );
    fs::remove_file(root.join("destination.db-wal")).expect("cleanup");
    drop(held);
}

/// A v2 database without `session_message` is incomplete: adoption refuses it
/// rather than recording a store whose message count is unknowable.
#[test]
fn adopt_refuses_a_v2_store_without_messages() {
    let (held, root) = fixture();
    let campaign = root.parent().expect("parent").to_path_buf();
    let campaign_arg = campaign.to_string_lossy().into_owned();
    let partial = campaign.join("partial-v2.db");
    let connection = Connection::open(&partial).expect("partial");
    connection
        .execute_batch("CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time INTEGER);")
        .expect("schema");
    drop(connection);
    let db_arg = partial.to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "adopt",
        "--root",
        &campaign_arg,
        "--name",
        "staging",
        "--db",
        &db_arg,
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        !campaign.join("adoption/staging.json").exists(),
        "a refused adoption writes no record"
    );
    drop(held);
}

/// A pinned tool importing an unpinned relative file is refused: the pin is a
/// closure, not a single entrypoint.
#[test]
fn install_refuses_an_unpinned_import() {
    let campaign = tempfile::tempdir().expect("temp directory");
    let tools = campaign.path().join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let script = tools.join("install.mjs");
    fs::write(
        &script,
        "import { x } from './unpinned.mjs';\nconsole.log(x);\n",
    )
    .expect("script");
    fs::write(tools.join("unpinned.mjs"), "export const x = 1;\n").expect("dep");
    let digest = sha256_file(&script);
    fs::write(
        campaign.path().join("tools.json"),
        serde_json::to_string(&json!({
            "tools": { "install.mjs": {
                "path": script.to_string_lossy(),
                "sha256": digest,
            } },
        }))
        .expect("pins serialize"),
    )
    .expect("pins");
    let campaign_arg = campaign.path().to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "install",
        "--root",
        &campaign_arg,
        "--dir",
        "/target",
        "--source",
        "/staged.db",
        "--state",
        "/state.json",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not pinned"), "unexpected stderr: {stderr}");
}

/// A tampered transitive import fails even when the entrypoint is untouched.
#[test]
fn install_refuses_a_tampered_transitive_import() {
    let campaign = tempfile::tempdir().expect("temp directory");
    let tools = campaign.path().join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let script = tools.join("install.mjs");
    fs::write(&script, "import { x } from './lib.mjs';\nconsole.log(x);\n").expect("script");
    let dep = tools.join("lib.mjs");
    fs::write(&dep, "export const x = 1;\n").expect("dep");
    let script_digest = sha256_file(&script);
    fs::write(
        campaign.path().join("tools.json"),
        serde_json::to_string(&json!({
            "tools": {
                "install.mjs": { "path": script.to_string_lossy(), "sha256": script_digest },
                "lib.mjs": { "path": dep.to_string_lossy(), "sha256": "0".repeat(64) },
            },
        }))
        .expect("pins serialize"),
    )
    .expect("pins");
    let campaign_arg = campaign.path().to_string_lossy().into_owned();

    let output = run(&[
        "sessions",
        "install",
        "--root",
        &campaign_arg,
        "--dir",
        "/target",
        "--source",
        "/staged.db",
        "--state",
        "/state.json",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to run"),
        "unexpected stderr: {stderr}"
    );
}
