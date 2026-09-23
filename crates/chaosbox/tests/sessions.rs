//! Receipt-chain resolution and verification failure behavior.
//!
//! The digest *encoder* is proved against JavaScript in
//! `sessions::digest`; this file covers everything around it: that the
//! journals resolve to exactly one receipt per session, that every way they
//! can fail to do so is a hard error rather than a silently wrong answer,
//! and that the CLI's exit codes follow from that.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chaosbox::sessions::{
    campaign::{Campaign, CampaignError},
    cli::{self, Command},
    digest::session_digest,
    verify::{verify, VerifyError, VerifyOptions},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use tempfile::TempDir;

/// Session id used by the fixture destination.
const SESSION: &str = "ses_fixture01";

/// Create a staging root holding `progress.json`, an empty journal, and a
/// destination database with one session and one message.
fn fixture() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path().join("staging");
    fs::create_dir_all(root.join("journal-v2")).expect("journal");
    fs::write(root.join("progress.json"), "{\"complete\": true}").expect("progress");
    build_destination(&root);
    (directory, root)
}

/// Create the destination database the receipts attest to.
fn build_destination(root: &Path) {
    let connection = Connection::open(root.join("destination.db")).expect("destination");
    connection
        .execute_batch(
            "CREATE TABLE session_v2 (
                id TEXT PRIMARY KEY,
                time INTEGER,
                cost REAL,
                summary TEXT
             );
             CREATE TABLE session_message (
                session_id TEXT,
                seq INTEGER,
                id TEXT,
                role TEXT,
                time INTEGER
             );
             CREATE TABLE project_directory (root TEXT);
             CREATE TABLE worktree (root TEXT);
             INSERT INTO session_v2 VALUES
                ('ses_fixture01', 1790166653727, 3.6143699999999996, 'a summary'),
                ('ses_fixture02', 1790166653727, 0.448854, 'another summary');
             INSERT INTO session_message VALUES
                ('ses_fixture01', 1, 'msg_1', 'user', 1790166653727),
                ('ses_fixture02', 1, 'msg_2', 'user', 1790166653727);",
        )
        .expect("schema");
}

/// Open the destination read-only, the way verification does.
fn destination(root: &Path) -> Connection {
    Connection::open_with_flags(
        root.join("destination.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("destination")
}

/// The destination digest of `session`, as a receipt would record it.
/// Encoding is proved separately against JavaScript, so the chain and
/// verification tests can build honest receipts from it.
fn digest_for(root: &Path, session: &str) -> String {
    session_digest(&destination(root), session)
        .expect("fixture digests")
        .digest
}

/// The destination digest of the fixture's primary session.
fn recorded_digest(root: &Path) -> String {
    digest_for(root, SESSION)
}

/// A receipt body for `session`, attesting to `digest`.
fn receipt(session: &str, digest: &str) -> Value {
    json!({
        "sessionID": session,
        "source": "primary",
        "destinationDigest": digest,
        "inputDigest": digest,
        "recoveryDigest": digest,
        "messages": 1,
    })
}

/// Write one receipt file into a journal.
fn write_receipt(root: &Path, journal: &str, file: &str, body: &Value) {
    let directory = root.join(journal);
    fs::create_dir_all(&directory).expect("journal");
    let text = serde_json::to_string(body).expect("receipt serializes");
    fs::write(directory.join(file), text).expect("receipt");
}

/// Verification options covering the whole campaign.
fn every_receipt() -> VerifyOptions {
    VerifyOptions {
        limit: 0,
        sessions: Vec::new(),
        sources: false,
        allow_partial: false,
    }
}

// ---- chain resolution ----

/// One receipt resolves to itself with depth one.
#[test]
fn a_single_receipt_is_its_own_effective_receipt() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "abc"));

    let effective = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect("chain resolves");

    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0].session, SESSION);
    assert_eq!(effective[0].depth, 1);
    assert_eq!(effective[0].head.file, "ses_fixture01.json");
    drop(held);
}

/// A three-deep chain resolves to the newest receipt, across journals, and
/// reports how much it superseded. This is the shape Phase 2 produces.
#[test]
fn a_supersession_chain_resolves_to_the_newest_receipt() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "stale"));

    let renewed = json!({
        "sessionID": SESSION,
        "destinationDigest": "newer",
        "messages": 1,
        "supersedes": "journal-v2/ses_fixture01.json",
    });
    write_receipt(&root, "journal-v3", "ses_fixture01.json", &renewed);

    let newest = json!({
        "sessionID": SESSION,
        "destinationDigest": digest,
        "messages": 1,
        "supersedes": "journal-v3/ses_fixture01.json",
    });
    write_receipt(&root, "journal-v3", "ses_fixture01.2.json", &newest);

    let effective = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect("chain resolves");

    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0].depth, 3);
    assert_eq!(effective[0].head.journal, "journal-v3");
    assert_eq!(effective[0].head.file, "ses_fixture01.2.json");
    assert_eq!(effective[0].head.destination_digest(), Some(digest.as_str()));
    drop(held);
}

/// A supersession naming a receipt that does not exist is an error, never a
/// silently shorter chain.
#[test]
fn a_dangling_supersession_is_rejected() {
    let (held, root) = fixture();
    let body = json!({
        "sessionID": SESSION,
        "destinationDigest": "abc",
        "messages": 1,
        "supersedes": "journal-v2/ses_absent.json",
    });
    write_receipt(&root, "journal-v3", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::DanglingSupersession { target, .. }
            if target == "journal-v2/ses_absent.json"),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A receipt may not replace a different session's receipt: that would let
/// one session's digest stand in for another's.
#[test]
fn a_foreign_supersession_is_rejected() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "abc"));
    write_receipt(&root, "journal-v2", "ses_other0009x.json", &receipt("ses_other0009x", "def"));

    let body = json!({
        "sessionID": "ses_other0009x",
        "destinationDigest": "ghi",
        "messages": 1,
        "supersedes": "journal-v2/ses_fixture01.json",
    });
    write_receipt(&root, "journal-v3", "ses_other0009x.2.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::ForeignSupersession { .. }),
        "unexpected error: {error}"
    );
    drop(held);
}

/// Two receipts nothing supersedes means no one receipt is authoritative.
#[test]
fn competing_unreferenced_receipts_are_ambiguous() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "one"));
    write_receipt(&root, "journal-v3", "ses_fixture01.json", &receipt(SESSION, "two"));

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::AmbiguousEffective { count: 2, .. }),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A cycle has no head, so there is no receipt to call effective.
#[test]
fn a_cyclic_chain_is_rejected() {
    let (held, root) = fixture();
    let forward = json!({
        "sessionID": SESSION,
        "destinationDigest": "abc",
        "messages": 1,
        "supersedes": "journal-v2/ses_fixture01.2.json",
    });
    let backward = json!({
        "sessionID": SESSION,
        "destinationDigest": "def",
        "messages": 1,
        "supersedes": "journal-v2/ses_fixture01.json",
    });
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &forward);
    write_receipt(&root, "journal-v2", "ses_fixture01.2.json", &backward);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::CyclicChain { .. }),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A receipt outside the chain would otherwise be quietly ignored, so the
/// walk has to prove it reached every receipt it started with.
#[test]
fn a_disconnected_chain_is_rejected() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "base"));

    let superseding = json!({
        "sessionID": SESSION,
        "destinationDigest": "abc",
        "messages": 1,
        "supersedes": "journal-v2/ses_fixture01.json",
    });
    write_receipt(&root, "journal-v3", "ses_fixture01.json", &superseding);

    // A self-superseding receipt: referenced, but unreachable from the head.
    let detached = json!({
        "sessionID": SESSION,
        "destinationDigest": "def",
        "messages": 1,
        "supersedes": "journal-v3/ses_fixture01.3.json",
    });
    write_receipt(&root, "journal-v3", "ses_fixture01.3.json", &detached);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::DisconnectedChain { visited: 2, expected: 3, .. }),
        "unexpected error: {error}"
    );
    drop(held);
}

/// The filename promises the session; the body has to agree, or a receipt
/// could be resolved under the wrong identity.
#[test]
fn a_receipt_whose_body_disagrees_with_its_filename_is_rejected() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt("ses_other0009x", "abc"));

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::SessionMismatch { expected, found, .. }
                if expected == SESSION && found == "ses_other0009x"
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// Malformed JSON names the file that failed rather than aborting vaguely.
#[test]
fn a_malformed_receipt_is_reported_with_its_path() {
    let (held, root) = fixture();
    fs::write(
        root.join("journal-v2/ses_fixture01.json"),
        "{not json",
    )
    .expect("receipt");

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .receipts()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::Parse { path, .. }
            if path.ends_with("ses_fixture01.json")),
        "unexpected error: {error}"
    );
    drop(held);
}

/// Anything that is not a staging root is refused before it is inspected.
#[test]
fn a_directory_without_the_staging_layout_is_rejected() {
    let directory = tempfile::tempdir().expect("temp directory");
    let error = Campaign::open(Some(directory.path().to_path_buf())).expect_err("must fail");
    assert!(
        matches!(&error, CampaignError::MissingLayout { .. }),
        "unexpected error: {error}"
    );
}

/// With no `--root` and no environment variable there is nothing to verify.
#[test]
fn a_missing_campaign_root_is_rejected() {
    // Poison the environment variable so the ambient value cannot leak in.
    std::env::remove_var("CHAOSBOX_SESSION_CAMPAIGN");
    let error = Campaign::open(None).expect_err("must fail");
    assert!(
        matches!(&error, CampaignError::MissingRoot),
        "unexpected error: {error}"
    );
}

// ---- verification behavior ----

/// A pass that checks every receipt and finds no disagreement succeeds.
#[test]
fn a_clean_complete_pass_succeeds() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, &digest));

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.sessions_effective, 1);
    assert_eq!(report.sessions_checked, 1);
    assert_eq!(report.sessions_verified, 1);
    assert!(report.complete, "the whole campaign was covered");
    assert!(report.clean(), "nothing disagreed: {report:?}");
    assert!(report.succeeded(false), "complete and clean");
    drop(held);
}

/// Bounding the pass means it cannot vouch for the sessions it skipped, so
/// `--allow-partial` is what lets an operator accept that deliberately.
#[test]
fn a_bounded_pass_is_incomplete_until_partial_is_allowed() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, &digest));
    let second = digest_for(&root, "ses_fixture02");
    write_receipt(&root, "journal-v2", "ses_fixture02.json", &receipt("ses_fixture02", &second));

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let bounded = verify(
        &campaign,
        &VerifyOptions { limit: 1, ..every_receipt() },
    )
    .expect("verifies");

    assert_eq!(bounded.sessions_checked, 1);
    assert!(!bounded.complete, "one of two receipts was checked");
    assert!(bounded.clean(), "the one it checked agreed: {bounded:?}");
    assert!(!bounded.succeeded(false), "a bounded pass cannot vouch for the rest");
    assert!(bounded.succeeded(true), "--allow-partial accepts a bounded clean pass");
    drop(held);
}

/// An honest receipt pointing at the wrong bytes has to fail the pass.
#[test]
fn a_digest_mismatch_fails_the_pass() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "deadbeef"));

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.sessions_checked, 1);
    assert_eq!(report.sessions_verified, 0);
    assert_eq!(report.digest_mismatch.len(), 1);
    assert_eq!(report.digest_mismatch[0].session, SESSION);
    assert_eq!(report.digest_mismatch[0].expected, "deadbeef");
    assert_ne!(report.digest_mismatch[0].actual, "deadbeef");
    assert!(!report.clean(), "a digest mismatch is a failure");
    assert!(!report.succeeded(true));
    drop(held);
}

/// A receipt attesting to more messages than survived is a failure too, and
/// is reported separately from a digest disagreement.
#[test]
fn a_message_count_mismatch_fails_the_pass() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    let mut body = receipt(SESSION, &digest);
    body["messages"] = json!(7);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.digest_mismatch.len(), 0, "the bytes still hash the same");
    assert_eq!(report.message_count_mismatch.len(), 1);
    assert_eq!(report.message_count_mismatch[0].expected, 7);
    assert_eq!(report.message_count_mismatch[0].actual, 1);
    assert!(!report.clean());
    drop(held);
}

/// A receipt for a session the destination never held is reported, not
/// skipped.
#[test]
fn a_session_missing_from_the_destination_is_reported() {
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture03.json", &receipt("ses_fixture03", "abc"));

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.sessions_checked, 1);
    assert_eq!(report.sessions_verified, 0);
    assert_eq!(report.missing, vec!["ses_fixture03".to_string()]);
    assert!(!report.clean());
    drop(held);
}

/// Asking for a session the campaign has no receipt for is an error: a
/// silently empty pass would look like success.
#[test]
fn an_unknown_session_is_an_error() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, &digest));

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let error = verify(
        &campaign,
        &VerifyOptions {
            sessions: vec!["ses_notpresent".to_string()],
            ..every_receipt()
        },
    )
    .expect_err("must fail");

    assert!(
        matches!(&error, VerifyError::UnknownSession(id) if id == "ses_notpresent"),
        "unexpected error: {error}"
    );
    drop(held);
}

/// Unresolvable receipts surface as chain errors on the report and make the
/// pass fail, rather than being treated as "no sessions to check".
#[test]
fn a_chain_error_makes_the_pass_fail() {
    let (held, root) = fixture();
    let body = json!({
        "sessionID": SESSION,
        "destinationDigest": "abc",
        "messages": 1,
        "supersedes": "journal-v2/ses_absent.json",
    });
    write_receipt(&root, "journal-v3", "ses_fixture01.json", &body);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("report still produced");

    assert_eq!(report.chain_errors.len(), 1);
    assert_eq!(report.sessions_effective, 0);
    assert!(!report.clean(), "a chain error is a failure");
    assert!(!report.succeeded(true), "not even --allow-partial rescues it");
    drop(held);
}

// ---- CLI exit codes ----

/// `status` reports and always exits zero.
#[test]
fn status_exits_zero() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, &digest));

    let code = cli::run(Command::Status { root: Some(root.clone()) }).expect("status runs");
    assert_eq!(code, 0);
    drop(held);
}

/// Exit codes follow the report: a complete clean pass is `0`, a bounded
/// clean pass is `1` until `--allow-partial` is passed, and a failure is
/// always `1`.
#[test]
fn verify_exit_codes_follow_the_report() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, &digest));
    let second = digest_for(&root, "ses_fixture02");
    write_receipt(&root, "journal-v2", "ses_fixture02.json", &receipt("ses_fixture02", &second));

    let verify_command = |limit: usize, allow_partial: bool| Command::Verify {
        root: Some(root.clone()),
        limit,
        sessions: Vec::new(),
        sources: false,
        allow_partial,
    };

    assert_eq!(cli::run(verify_command(0, false)).expect("complete pass"), 0);
    assert_eq!(cli::run(verify_command(1, false)).expect("bounded pass"), 1);
    assert_eq!(cli::run(verify_command(1, true)).expect("bounded allowed"), 0);
    drop(held);

    // A failing destination exits 1 even with --allow-partial.
    let (held, root) = fixture();
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &receipt(SESSION, "deadbeef"));
    let failing = Command::Verify {
        root: Some(root),
        limit: 0,
        sessions: Vec::new(),
        sources: false,
        allow_partial: true,
    };
    assert_eq!(cli::run(failing).expect("failing pass"), 1);
    drop(held);
}

/// The CLI reports campaign problems as an error rather than an exit code,
/// so the caller can tell "failed verification" from "could not run".
#[test]
fn a_broken_campaign_is_an_error_not_an_exit_code() {
    let directory = tempfile::tempdir().expect("temp directory");
    let error = cli::run(Command::Status {
        root: Some(directory.path().to_path_buf()),
    })
    .expect_err("must fail");
    assert!(error.contains("missing"), "unexpected error: {error}");
}
