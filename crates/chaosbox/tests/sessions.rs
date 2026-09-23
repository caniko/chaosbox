//! Receipt-chain resolution, receipt schema, inventory reconciliation, and
//! verification failure behavior.
//!
//! The digest *encoder* is proved against JavaScript in `sessions::digest`;
//! this file covers everything around it: that the journals resolve to
//! exactly one receipt per session, that a receipt has to satisfy the schema
//! before anything reads it, that a pass is measured against the inventory
//! the driver pinned rather than against whatever the journals happen to
//! hold, that every way those can fail is a hard failure rather than a
//! silently wrong answer, and that the CLI's exit codes follow from that.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use chaosbox::sessions::{
    campaign::{Campaign, CampaignError},
    cli::{self, Command},
    digest::{recovered_hash, session_digest},
    verify::{verify, VerifyError, VerifyOptions},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Session id the fixture destination holds by default.
const SESSION: &str = "ses_fixture01";
/// Second session, added by passes that need to bound themselves.
const OTHER: &str = "ses_fixture02";
/// A session the destination never held.
const ABSENT: &str = "ses_fixture09";

/// Create a staging root holding an empty inventory, an empty journal, and a
/// destination database with one session and one message.
fn fixture() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temp directory");
    let root = directory.path().join("staging");
    fs::create_dir_all(root.join("journal-v2")).expect("journal");
    build_destination(&root);
    declare(&root, &[]);
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
                ('ses_fixture01', 1790166653727, 3.6143699999999996, 'a summary');
             INSERT INTO session_message VALUES
                ('ses_fixture01', 1, 'msg_1', 'user', 1790166653727);",
        )
        .expect("schema");
}

/// Add one more session and its message to the destination, so a pass has
/// something to bound itself across.
fn add_session(root: &Path, id: &str) {
    let connection = Connection::open(root.join("destination.db")).expect("destination");
    connection
        .execute(
            "INSERT INTO session_v2 VALUES (?1, 1790166653727, 0.448854, 'another summary')",
            [id],
        )
        .expect("session");
    connection
        .execute(
            "INSERT INTO session_message VALUES (?1, 1, 'msg_' || ?1, 'user', 1790166653727)",
            [id],
        )
        .expect("message");
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

/// Lowercase hex SHA-256 over `text`, the shape a receipt's digests must
/// have. Values that already look like a digest pass through untouched, so a
/// test can hand in a real recomputed digest or a short readable label and
/// get a contract-satisfying field either way.
fn sha(text: &str) -> String {
    if text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return text.to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// A base receipt for `session`, attesting to `digest`.
fn receipt(session: &str, digest: &str) -> Value {
    json!({
        "sessionID": session,
        "source": "primary",
        "destinationDigest": sha(digest),
        "inputDigest": sha(digest),
        "recoveryDigest": sha(digest),
        "messages": 1,
        "transformation": "interrupted-draft-v1",
        "drafts": 0,
    })
}

/// A receipt that replaces `target`: the same attestation plus the address
/// of the receipt it supersedes.
fn superseding(session: &str, digest: &str, target: &str) -> Value {
    let mut body = receipt(session, digest);
    body["supersedes"] = json!(target);
    body
}

/// Write one receipt file into a journal.
fn write_receipt(root: &Path, journal: &str, file: &str, body: &Value) {
    let directory = root.join(journal);
    fs::create_dir_all(&directory).expect("journal");
    let text = serde_json::to_string(body).expect("receipt serializes");
    fs::write(directory.join(file), text).expect("receipt");
}

/// Record the sessions the driver claims it migrated, so a pass has an
/// inventory that comes from outside the receipts it is about to check.
fn declare(root: &Path, ids: &[&str]) {
    let verified: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "source": "primary",
                "messages": 1,
                "drafts": 0,
                "verification": "journal digest revalidated",
            })
        })
        .collect();
    let progress = json!({
        "total": ids.len(),
        "verified": verified,
        "deferred": [],
        "errors": [],
        "complete": true,
        "identityDigest": sha("identity"),
        "driverDigest": sha("driver"),
    });
    fs::write(root.join("progress.json"), progress.to_string()).expect("progress");
}

/// Write `progress.json` verbatim, for shapes the inventory parser has to
/// refuse or treat specially.
fn write_progress(root: &Path, progress: &Value) {
    fs::write(
        root.join("progress.json"),
        serde_json::to_string(progress).expect("progress serializes"),
    )
    .expect("progress");
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
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "abc"),
    );

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
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "stale"),
    );
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.json",
        &superseding(SESSION, "newer", "journal-v2/ses_fixture01.json"),
    );
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.2.json",
        &superseding(SESSION, &digest, "journal-v3/ses_fixture01.json"),
    );

    let effective = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect("chain resolves");

    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0].depth, 3);
    assert_eq!(effective[0].head.journal, "journal-v3");
    assert_eq!(effective[0].head.file, "ses_fixture01.2.json");
    assert_eq!(
        effective[0].head.destination_digest(),
        Some(digest.as_str())
    );
    drop(held);
}

/// A supersession naming a receipt that does not exist is an error, never a
/// silently shorter chain.
#[test]
fn a_dangling_supersession_is_rejected() {
    let (held, root) = fixture();
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.json",
        &superseding(SESSION, "abc", "journal-v2/ses_absent.json"),
    );

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
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "abc"),
    );
    write_receipt(
        &root,
        "journal-v2",
        "ses_other0009x.json",
        &receipt("ses_other0009x", "def"),
    );
    write_receipt(
        &root,
        "journal-v3",
        "ses_other0009x.2.json",
        &superseding("ses_other0009x", "ghi", "journal-v2/ses_fixture01.json"),
    );

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
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "one"),
    );
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.json",
        &receipt(SESSION, "two"),
    );

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
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &superseding(SESSION, "abc", "journal-v2/ses_fixture01.2.json"),
    );
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.2.json",
        &superseding(SESSION, "def", "journal-v2/ses_fixture01.json"),
    );

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
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "base"),
    );
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.json",
        &superseding(SESSION, "abc", "journal-v2/ses_fixture01.json"),
    );

    // A self-superseding receipt: referenced, but unreachable from the head.
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.3.json",
        &superseding(SESSION, "def", "journal-v3/ses_fixture01.3.json"),
    );

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::DisconnectedChain {
                visited: 2,
                expected: 3,
                ..
            }
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// The filename promises the session; the body has to agree, or a receipt
/// could be resolved under the wrong identity.
#[test]
fn a_receipt_whose_body_disagrees_with_its_filename_is_rejected() {
    let (held, root) = fixture();
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt("ses_other0009x", "abc"),
    );

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

// ---- receipt schema ----

/// An absent field and a mistyped one have to read differently. `supersedes`
/// is the one that matters: a value of the wrong type used to be read as
/// "absent", which turned a supersession into a base receipt carrying a
/// second, competing set of attestation fields.
#[test]
fn a_mistyped_supersedes_is_rejected_instead_of_being_read_as_absent() {
    let (held, root) = fixture();
    let mut body = receipt(SESSION, "abc");
    body["supersedes"] = json!(42);
    write_receipt(&root, "journal-v3", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .effective_receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::InvalidField { field, .. } if field == "supersedes"
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A receipt that does not say where it came from is not attesting to
/// anything a source pass could check, so it is refused outright rather than
/// skipped later.
#[test]
fn a_receipt_missing_required_attestation_is_rejected() {
    let (held, root) = fixture();
    let mut body = receipt(SESSION, "abc");
    body.as_object_mut()
        .expect("object")
        .remove("source")
        .expect("source was there");
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::MissingField { field, .. } if field == "source"
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A session with no identity at all is a different failure from one whose
/// identity disagrees with its file name.
#[test]
fn a_missing_session_id_is_distinct_from_a_mismatched_one() {
    let (held, root) = fixture();
    let mut body = receipt(SESSION, "abc");
    body.as_object_mut()
        .expect("object")
        .remove("sessionID")
        .expect("sessionID was there");
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::MissingField { field, .. } if field == "sessionID"
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A digest that is not 64 hex digits cannot be the output of SHA-256, so
/// comparing it would be theatre.
#[test]
fn a_receipt_with_a_malformed_digest_is_rejected() {
    let (held, root) = fixture();
    let mut body = receipt(SESSION, "abc");
    body["destinationDigest"] = json!("deadbeef");
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::InvalidField { field, problem, .. }
                if field == "destinationDigest" && problem.contains("hex")
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// A negative count means the receipt was not written by anything that
/// counted the rows.
#[test]
fn a_negative_message_count_is_rejected() {
    let (held, root) = fixture();
    let mut body = receipt(SESSION, "abc");
    body["messages"] = json!(-1);
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::InvalidField { field, .. } if field == "messages"
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// `source` is joined into a path to find the snapshot. A receipt that
/// smuggled a separator in there would aim verification at any `.db` file on
/// the machine, so the name has to be a bare database name.
#[test]
fn a_source_name_that_would_escape_the_campaign_is_rejected() {
    let (held, root) = fixture();
    let mut body = receipt(SESSION, "abc");
    body["source"] = json!("../../secrets");
    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .receipts()
        .expect_err("must fail");

    assert!(
        matches!(
            &error,
            CampaignError::InvalidField { field, .. } if field == "source"
        ),
        "unexpected error: {error}"
    );
    drop(held);
}

/// Malformed JSON names the file that failed rather than aborting vaguely.
#[test]
fn a_malformed_receipt_is_reported_with_its_path() {
    let (held, root) = fixture();
    fs::write(root.join("journal-v2/ses_fixture01.json"), "{not json").expect("receipt");

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

// ---- inventory reconciliation ----

/// An empty journal cannot pass over an inventory that is not empty.
///
/// This is the false success the whole inventory exists to prevent: with no
/// receipts to disagree with, a pass that measured the destination only
/// against what it discovered would report a clean, complete, vacuous win.
#[test]
fn an_empty_journal_cannot_pass_over_an_inventory_that_is_not_empty() {
    let (held, root) = fixture();
    declare(&root, &[SESSION, OTHER]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("report still produced");

    assert_eq!(report.sessions_effective, 0, "there are no receipts at all");
    assert_eq!(report.inventory.expected, 2);
    assert_eq!(report.inventory.receipts, 0);
    assert_eq!(report.inventory.missing_receipts.len(), 2);
    assert!(!report.inventory.reconciled, "expected but never attested");
    assert!(!report.clean(), "an empty journal is not a clean pass");
    assert!(
        !report.succeeded(true),
        "not even --accept-partial rescues a pass over nothing"
    );
    assert!(
        !report.succeeded(false),
        "a vacuous pass can never claim completeness"
    );
    drop(held);
}

/// A receipt the driver's own inventory never sanctioned is the opposite
/// outcome from a missing one, and has to be reported as its own thing.
#[test]
fn a_receipt_the_inventory_never_sanctioned_is_reported() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(
        report.inventory.unexpected_receipts,
        vec![SESSION.to_string()]
    );
    assert!(report.inventory.missing_receipts.is_empty());
    assert!(!report.inventory.reconciled);
    assert!(!report.clean());
    drop(held);
}

/// A destination row nothing accounts for is the third distinct outcome: the
/// database holds a session no receipt and no inventory entry explains.
#[test]
fn a_destination_row_no_receipt_accounts_for_is_reported() {
    let (held, root) = fixture();
    add_session(&root, OTHER);
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(
        report.inventory.unexpected_destination,
        vec![OTHER.to_string()],
        "the destination holds a second session nobody attested to"
    );
    assert!(report.inventory.absent_destination.is_empty());
    assert!(!report.inventory.reconciled);
    assert!(!report.clean());
    drop(held);
}

/// A receipt with no row behind it is the fourth outcome, and it is checked
/// over the whole inventory rather than only the sessions selected.
#[test]
fn a_receipt_with_no_destination_row_is_reported_across_the_whole_inventory() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture09.json",
        &receipt(ABSENT, "abc"),
    );
    declare(&root, &[SESSION, ABSENT]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(
        &campaign,
        // Bound the pass to one session: reconciliation still has to see the
        // receipt it never selected.
        &VerifyOptions {
            limit: 1,
            ..every_receipt()
        },
    )
    .expect("verifies");

    assert_eq!(
        report.inventory.absent_destination,
        vec![ABSENT.to_string()],
        "reconciliation does not depend on which sessions were selected"
    );
    assert!(report.inventory.missing_receipts.is_empty());
    assert!(!report.inventory.reconciled);
    assert!(!report.clean());
    drop(held);
}

/// Work the driver deliberately skipped still means the campaign is not
/// finished, however well the receipts reconcile.
#[test]
fn a_deferred_session_fails_reconciliation() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    write_progress(
        &root,
        &json!({
            "total": 1,
            "verified": [{"id": SESSION}],
            "deferred": [{"id": OTHER}],
            "errors": [],
            "complete": true,
        }),
    );

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.inventory.deferred, 1);
    assert!(!report.inventory.reconciled, "the driver left work behind");
    assert!(!report.clean());
    drop(held);
}

/// `total` disagreeing with the entries actually listed means the inventory
/// does not describe itself, so it cannot describe the campaign either.
#[test]
fn an_inventory_whose_total_disagrees_with_its_entries_is_not_reconciled() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    write_progress(
        &root,
        &json!({
            "total": 5,
            "verified": [{"id": SESSION}],
            "deferred": [],
            "errors": [],
            "complete": true,
        }),
    );

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.inventory.total, 5);
    assert_eq!(report.inventory.expected, 1);
    assert!(!report.inventory.reconciled);
    assert!(!report.clean());
    drop(held);
}

/// `progress.json` that does not describe an inventory is reported as such.
/// The pass still produces a report — an operator wants the destination's
/// integrity results even when the campaign's own bookkeeping is broken —
/// but it can never claim to have measured anything.
#[test]
fn an_unreadable_inventory_is_reported_instead_of_invented() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    write_progress(&root, &json!({ "verified": "not an array" }));

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("report still produced");

    let error = report.inventory.error.as_deref().expect("reported");
    assert!(error.contains("verified"), "unexpected error: {error}");
    assert!(!report.inventory.reconciled);
    assert!(!report.clean());
    assert!(!report.succeeded(true), "no inventory, no success");
    drop(held);
}

/// Two entries for one session is not an inventory; it is a double count.
#[test]
fn an_inventory_listing_a_session_twice_is_refused() {
    let (held, root) = fixture();
    write_progress(
        &root,
        &json!({
            "total": 2,
            "verified": [{"id": SESSION}, {"id": SESSION}],
            "deferred": [],
            "errors": [],
            "complete": true,
        }),
    );

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .inventory()
        .expect_err("must fail");

    assert!(
        matches!(&error, CampaignError::InvalidProgress { field, .. } if field.contains("verified")),
        "unexpected error: {error}"
    );
    drop(held);
}

/// The counters are the reason an external inventory is worth having: they
/// are how a pass learns the driver reported no leftovers. Written as
/// defaults they manufacture the very agreement they are supposed to
/// measure — `total` defaulted to `verified.len()` matches by construction
/// and `deferred`/`errors` defaulted to zero say nothing was left behind —
/// so all three are required instead.
///
/// Every set difference here is empty: the receipt, the destination and the
/// declared sessions all agree. Only the missing counters stand between this
/// campaign and a clean pass, and `--allow-partial` must not move them.
#[test]
fn an_inventory_missing_its_counters_is_refused_even_when_partial() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    let full = json!({
        "total": 1,
        "verified": [{"id": SESSION}],
        "deferred": [],
        "errors": [],
        "complete": true,
        "identityDigest": sha("identity"),
        "driverDigest": sha("driver"),
    });

    for omitted in ["total", "deferred", "errors"] {
        let mut progress = full.clone();
        progress
            .as_object_mut()
            .expect("progress is an object")
            .remove(omitted);
        write_progress(&root, &progress);

        let error = Campaign::open(Some(root.clone()))
            .expect("campaign")
            .inventory()
            .expect_err("a counter nobody wrote is not a counter");
        assert!(
            matches!(&error, CampaignError::InvalidProgress { field, .. } if field == omitted),
            "omitting `{omitted}` gave: {error}"
        );

        let campaign = Campaign::open(Some(root.clone())).expect("campaign");
        let report = verify(&campaign, &every_receipt()).expect("report still produced");
        assert!(
            !report.inventory.reconciled,
            "`{omitted}` was invented on the driver's behalf"
        );
        assert!(!report.clean());
        assert!(
            !report.succeeded(true),
            "`{omitted}` absent, yet --allow-partial passed"
        );
    }
    drop(held);
}

/// `complete` is the driver's own claim to have finished. Absent reads as
/// "not finished", which the report shows; present with the wrong type must
/// not quietly become `false`, or a mistyped campaign hides behind
/// `--allow-partial` looking merely incomplete.
#[test]
fn a_mistyped_completion_is_refused_rather_than_read_as_false() {
    let (held, root) = fixture();
    write_progress(
        &root,
        &json!({
            "total": 0,
            "verified": [],
            "deferred": [],
            "errors": [],
            "complete": "true",
        }),
    );

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .inventory()
        .expect_err("a string is not a completion state");
    assert!(
        matches!(&error, CampaignError::InvalidProgress { field, .. } if field == "complete"),
        "unexpected error: {error}"
    );
    drop(held);
}

/// The campaign digests are echoed into the report as the campaign's own
/// assertions, so a value that is not a digest must never leave the parser
/// wearing the same shape as a real one.
#[test]
fn a_present_but_malformed_campaign_digest_is_refused() {
    let (held, root) = fixture();
    write_progress(
        &root,
        &json!({
            "total": 0,
            "verified": [],
            "deferred": [],
            "errors": [],
            "complete": true,
            "identityDigest": sha("identity"),
            "driverDigest": "62f6fb35",
        }),
    );

    let error = Campaign::open(Some(root.clone()))
        .expect("campaign")
        .inventory()
        .expect_err("a short hex string is not a digest");
    assert!(
        matches!(&error, CampaignError::InvalidProgress { field, .. } if field == "driverDigest"),
        "unexpected error: {error}"
    );
    drop(held);
}

// ---- verification behavior ----

/// A pass that checks every receipt and finds no disagreement succeeds, and
/// says which inventory it reconciled against.
#[test]
fn a_clean_complete_pass_succeeds() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.sessions_effective, 1);
    assert_eq!(report.sessions_checked, 1);
    assert_eq!(report.sessions_verified, 1);
    assert!(report.complete, "the whole campaign was covered");
    assert!(report.inventory.reconciled, "inventory matches: {report:?}");
    assert_eq!(report.inventory.expected, 1);
    assert_eq!(report.inventory.destination_rows, 1);
    assert!(report.snapshot.consistent_read, "reads ran in one snapshot");
    assert!(report.clean(), "nothing disagreed: {report:?}");
    assert!(report.succeeded(false), "complete and clean");
    drop(held);
}

/// Bounding the pass means it cannot vouch for the sessions it skipped, so
/// `--allow-partial` is what lets an operator accept that deliberately.
#[test]
fn a_bounded_pass_is_incomplete_until_partial_is_allowed() {
    let (held, root) = fixture();
    add_session(&root, OTHER);
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    let second = digest_for(&root, OTHER);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture02.json",
        &receipt(OTHER, &second),
    );
    declare(&root, &[SESSION, OTHER]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let bounded = verify(
        &campaign,
        &VerifyOptions {
            limit: 1,
            ..every_receipt()
        },
    )
    .expect("verifies");

    assert_eq!(bounded.sessions_checked, 1);
    assert!(bounded.inventory.reconciled, "reconciliation is complete");
    assert!(
        !bounded.complete,
        "one of two receipts was checked, so the pass cannot claim the other"
    );
    assert!(bounded.clean(), "the one it checked agreed: {bounded:?}");
    assert!(
        !bounded.succeeded(false),
        "a bounded pass cannot vouch for the rest"
    );
    assert!(
        bounded.succeeded(true),
        "--allow-partial accepts a bounded clean pass"
    );
    drop(held);
}

/// An honest receipt pointing at the wrong bytes has to fail the pass.
#[test]
fn a_digest_mismatch_fails_the_pass() {
    let (held, root) = fixture();
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "deadbeef"),
    );
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.sessions_checked, 1);
    assert_eq!(report.sessions_verified, 0);
    assert_eq!(report.digest_mismatch.len(), 1);
    assert_eq!(report.digest_mismatch[0].session, SESSION);
    assert_eq!(report.digest_mismatch[0].expected, sha("deadbeef"));
    assert_ne!(report.digest_mismatch[0].actual, sha("deadbeef"));
    assert!(report.inventory.reconciled, "only the digest disagrees");
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
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(
        report.digest_mismatch.len(),
        0,
        "the bytes still hash the same"
    );
    assert_eq!(report.message_count_mismatch.len(), 1);
    assert_eq!(report.message_count_mismatch[0].expected, 7);
    assert_eq!(report.message_count_mismatch[0].actual, 1);
    assert!(report.inventory.reconciled, "only the count disagrees");
    assert!(!report.clean());
    drop(held);
}

/// A receipt for a session the destination never held is reported, not
/// skipped, and shows up in the inventory reconciliation too.
#[test]
fn a_session_missing_from_the_destination_is_reported() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture09.json",
        &receipt(ABSENT, "abc"),
    );
    declare(&root, &[SESSION, ABSENT]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert_eq!(report.sessions_checked, 2);
    assert_eq!(report.sessions_verified, 1);
    assert_eq!(report.missing, vec![ABSENT.to_string()]);
    assert_eq!(
        report.inventory.absent_destination,
        vec![ABSENT.to_string()]
    );
    assert!(!report.clean());
    drop(held);
}

/// Asking for a session the campaign has no receipt for is an error: a
/// silently empty pass would look like success.
#[test]
fn an_unknown_session_is_an_error() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[SESSION]);

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
    write_receipt(
        &root,
        "journal-v3",
        "ses_fixture01.json",
        &superseding(SESSION, "abc", "journal-v2/ses_absent.json"),
    );
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("report still produced");

    assert_eq!(report.chain_errors.len(), 1);
    assert_eq!(report.sessions_effective, 0);
    assert!(!report.clean(), "a chain error is a failure");
    assert!(
        !report.succeeded(true),
        "not even --allow-partial rescues it"
    );
    drop(held);
}

/// Without `--sources`, the source and recovery digests are not part of the
/// claim — and the report has to say so rather than looking complete.
#[test]
fn source_digests_are_reported_as_unchecked_unless_requested() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(&campaign, &every_receipt()).expect("verifies");

    assert!(!report.checked_sources, "sources were never requested");
    assert_eq!(report.source_coverage, 0);
    assert!(report.clean(), "the destination digest is still attested");
    drop(held);
}

// ---- source coverage ----

/// Build the two snapshots a `--sources` pass reads: the source database the
/// `inputDigest` covers, and the recovered-rows snapshot behind
/// `recoveryDigest`. `source_has_session` decides whether the source still
/// holds the fixture session, so one test can cover a source that dropped it.
fn build_sources(root: &Path, source_has_session: bool) {
    let parent = root.parent().expect("staging root has a parent");
    fs::create_dir_all(parent.join("work")).expect("work directory");

    let source = Connection::open(parent.join("work/primary.db")).expect("source");
    source
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
             );",
        )
        .expect("source schema");
    if source_has_session {
        source
            .execute_batch(
                "INSERT INTO session_v2 VALUES
                    ('ses_fixture01', 1790166653727, 3.6143699999999996, 'a summary');
                 INSERT INTO session_message VALUES
                    ('ses_fixture01', 1, 'msg_1', 'user', 1790166653727);",
            )
            .expect("source session");
    }
    drop(source);

    let recovery = Connection::open(parent.join("recovered-rows.db")).expect("recovery");
    recovery
        .execute_batch(
            "CREATE TABLE recovered (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                text TEXT
             );
             INSERT INTO recovered VALUES
                ('r1', 'ses_fixture01', 'recovered text');",
        )
        .expect("recovery schema");
}

/// `--sources` has to cover every session it checked. Partial coverage would
/// read as a pass over sessions it never actually looked at.
#[test]
fn a_source_pass_covers_every_session_it_checks() {
    let (held, root) = fixture();
    build_sources(&root, true);
    let parent = root.parent().expect("parent").to_path_buf();

    let source_connection = Connection::open_with_flags(
        parent.join("work/primary.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("source");
    let recovery_connection = Connection::open_with_flags(
        parent.join("recovered-rows.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("recovery");

    let mut body = receipt(SESSION, &recorded_digest(&root));
    body["inputDigest"] = json!(
        session_digest(&source_connection, SESSION)
            .expect("source digest")
            .digest
    );
    body["recoveryDigest"] =
        json!(recovered_hash(&recovery_connection, SESSION).expect("recovery digest"));
    drop(source_connection);
    drop(recovery_connection);

    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(
        &campaign,
        &VerifyOptions {
            sources: true,
            ..every_receipt()
        },
    )
    .expect("verifies");

    assert!(
        report.checked_sources,
        "the pass was asked to check sources"
    );
    assert_eq!(report.sessions_checked, 1);
    assert_eq!(
        report.source_coverage, report.sessions_checked,
        "every checked session had both digests recomputed: {report:?}"
    );
    assert!(report.sources_uncovered.is_empty(), "{report:?}");
    assert!(report.source_mismatch.is_empty(), "{report:?}");
    assert!(report.recovery_mismatch.is_empty(), "{report:?}");
    assert!(report.source_errors.is_empty(), "{report:?}");
    assert!(report.clean(), "sources agree too: {report:?}");
    assert!(report.succeeded(false));
    drop(held);
}

/// A source pass over a snapshot that no longer holds the session is a
/// failure, reported separately from a digest that disagrees.
#[test]
fn a_source_missing_the_session_is_an_error_not_a_skip() {
    let (held, root) = fixture();
    build_sources(&root, false);
    let parent = root.parent().expect("parent").to_path_buf();

    let recovery_connection = Connection::open_with_flags(
        parent.join("recovered-rows.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("recovery");

    let mut body = receipt(SESSION, &recorded_digest(&root));
    body["recoveryDigest"] =
        json!(recovered_hash(&recovery_connection, SESSION).expect("recovery digest"));
    drop(recovery_connection);
    // The source snapshot exists but no longer holds this session, so
    // `inputDigest` cannot be recomputed at all.

    write_receipt(&root, "journal-v2", "ses_fixture01.json", &body);
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let report = verify(
        &campaign,
        &VerifyOptions {
            sources: true,
            ..every_receipt()
        },
    )
    .expect("verifies");

    assert_eq!(report.source_errors, vec![SESSION.to_string()]);
    assert!(!report.clean(), "an unrecoverable source is a failure");
    drop(held);
}

/// Asking `--sources` for a recovery snapshot that is not there is a hard
/// error: the pass cannot be half a source pass.
#[test]
fn a_source_pass_without_its_snapshots_cannot_run() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[SESSION]);

    let campaign = Campaign::open(Some(root.clone())).expect("campaign");
    let error = verify(
        &campaign,
        &VerifyOptions {
            sources: true,
            ..every_receipt()
        },
    )
    .expect_err("must fail");

    assert!(
        matches!(&error, VerifyError::Open { .. }),
        "unexpected error: {error}"
    );
    drop(held);
}

// ---- CLI exit codes ----

/// `status` reports and always exits zero.
#[test]
fn status_exits_zero() {
    let (held, root) = fixture();
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    declare(&root, &[SESSION]);

    let code = cli::run(Command::Status {
        root: Some(root.clone()),
    })
    .expect("status runs");
    assert_eq!(code, 0);
    drop(held);
}

/// Exit codes follow the report: a complete clean pass is `0`, a bounded
/// clean pass is `1` until `--allow-partial` is passed, and a failure is
/// always `1`.
#[test]
fn verify_exit_codes_follow_the_report() {
    let (held, root) = fixture();
    add_session(&root, OTHER);
    let digest = recorded_digest(&root);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, &digest),
    );
    let second = digest_for(&root, OTHER);
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture02.json",
        &receipt(OTHER, &second),
    );
    declare(&root, &[SESSION, OTHER]);

    let verify_command = |limit: usize, allow_partial: bool| Command::Verify {
        root: Some(root.clone()),
        limit,
        sessions: Vec::new(),
        sources: false,
        allow_partial,
    };

    assert_eq!(
        cli::run(verify_command(0, false)).expect("complete pass"),
        0
    );
    assert_eq!(cli::run(verify_command(1, false)).expect("bounded pass"), 1);
    assert_eq!(
        cli::run(verify_command(1, true)).expect("bounded allowed"),
        0
    );
    drop(held);

    // A failing destination exits 1 even with --allow-partial.
    let (held, root) = fixture();
    write_receipt(
        &root,
        "journal-v2",
        "ses_fixture01.json",
        &receipt(SESSION, "deadbeef"),
    );
    declare(&root, &[SESSION]);
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

/// A campaign whose inventory does not reconcile exits `1` even when
/// `--allow-partial` is passed: accepting a *bounded* pass is not the same
/// as accepting a pass over nothing.
#[test]
fn a_non_reconciling_inventory_exits_one_even_with_allow_partial() {
    let (held, root) = fixture();
    declare(&root, &[SESSION, OTHER]);

    let code = cli::run(Command::Verify {
        root: Some(root),
        limit: 0,
        sessions: Vec::new(),
        sources: false,
        allow_partial: true,
    })
    .expect("pass runs");
    assert_eq!(code, 1, "an empty journal over a populated inventory fails");
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
