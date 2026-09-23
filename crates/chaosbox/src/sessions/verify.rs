//! Read-only verification of a campaign against its destination database.
//!
//! A pass answers one question — *does the destination still hold exactly
//! what the receipts attest to* — and it has to answer it without being able
//! to talk itself into "yes" over nothing. Three things pin that down, and
//! `docs/SESSION_VERIFICATION.md` writes each one out:
//!
//! 1. **An inventory from outside the receipts.** `progress.json` lists every
//!    session the driver intended to migrate, so an empty journal cannot
//!    pass by having nothing to disagree with.
//! 2. **One snapshot per database.** Every read runs inside a single SQLite
//!    read transaction, so `quick_check`, the foreign-key walk, and all 7644
//!    digests describe the same bytes rather than whatever each query saw.
//! 3. **Stated coverage.** The report says which inventory, which
//!    properties, and how many of the selected sessions it actually
//!    recomputed — a bounded pass cannot claim the sessions it skipped.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    time::Instant,
};

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::sessions::{
    campaign::{Campaign, CampaignError, Effective},
    digest::{recovered_hash, session_digest, DigestError},
    inventory::Inventory,
    remap::{
        is_native_message_shape, mapping_variants_digest, message_map_digest,
        parse_mapping_variants, variant_message_id, variant_session_id, VariantEntry,
    },
};

/// What one verification pass should cover.
#[derive(Clone, Debug, Default)]
pub struct VerifyOptions {
    /// Sessions to recompute; `0` means every effective receipt.
    pub limit: usize,
    /// Restrict the pass to these session ids instead of the full journal.
    pub sessions: Vec<String>,
    /// Also recompute source and recovery digests against the snapshots.
    pub sources: bool,
    /// Treat a bounded pass with no failures as a success.
    pub allow_partial: bool,
}

/// A session whose destination digest disagreed with its effective receipt.
#[derive(Clone, Debug, Serialize)]
pub struct DigestMismatch {
    /// Session whose digest changed.
    pub session: String,
    /// Digest the effective receipt attests to.
    pub expected: String,
    /// Digest recomputed from the destination.
    pub actual: String,
}

/// A session whose message count disagreed with its effective receipt.
#[derive(Clone, Debug, Serialize)]
pub struct CountMismatch {
    /// Session whose message count changed.
    pub session: String,
    /// Count the effective receipt attests to.
    pub expected: i64,
    /// Count recomputed from the destination.
    pub actual: i64,
}

/// A variant receipt whose re-key proof failed: which check caught it and
/// how. Every check re-derives from receipt fields and database row order,
/// never from the mapping the driver wrote — the mapping pin is verified
/// separately, once, before any receipt is checked against it.
#[derive(Clone, Debug, Serialize)]
pub struct RemapMismatch {
    /// Variant session that failed re-derivation.
    pub session: String,
    /// Check that failed: `mapping-entry`, `session-id`, `message-map`,
    /// `message-pair`, `transformation`, or `receipt-field`.
    pub check: String,
    /// What disagreed.
    pub detail: String,
}

/// How the pinned inventory reconciles against the receipts and the
/// destination rows actually present.
///
/// Four set differences, each a distinct outcome, because they fail for
/// different reasons and an operator has to know which one happened:
/// `missing_receipts` means the driver never wrote an attestation,
/// `absent_destination` means an attestation has nothing behind it,
/// `unexpected_destination` means the destination holds a session nobody
/// attested to, and `unexpected_receipts` means a receipt exists that the
/// driver's own inventory never sanctioned.
#[derive(Clone, Debug, Default, Serialize)]
pub struct InventoryReport {
    /// What the driver expected to migrate, from `progress.total`.
    pub total: usize,
    /// Sessions listed in `progress.verified`.
    pub expected: usize,
    /// Sessions with an effective receipt in the journals.
    pub receipts: usize,
    /// Sessions with a row in the destination's `session_v2`.
    pub destination_rows: usize,
    /// Sessions the driver deliberately left behind.
    pub deferred: usize,
    /// Sessions the driver could not migrate.
    pub errors: usize,
    /// Variant sessions the pinned mapping sanctions, from
    /// `identity-v3.json`'s `mappingDigest`. Zero before G3.
    pub variants_expected: usize,
    /// Delta-new sessions the pinned delta inventory sanctions. Zero until
    /// the delta identity lands.
    pub delta_expected: usize,
    /// Whether the driver finished the run that pinned this inventory.
    pub progress_complete: bool,
    /// Digest pinning the campaign identity, when `progress.json` recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_digest: Option<String>,
    /// Digest pinning the frozen migration script.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_digest: Option<String>,
    /// Whether the inventory, the receipts, and the destination agree
    /// completely and the driver left nothing deferred or failed.
    pub reconciled: bool,
    /// Expected sessions with no receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_receipts: Vec<String>,
    /// Receipts for sessions the inventory never sanctioned.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unexpected_receipts: Vec<String>,
    /// Receipts with no row in the destination, across the whole inventory
    /// rather than only the sessions this pass selected.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub absent_destination: Vec<String>,
    /// Destination rows no receipt accounts for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unexpected_destination: Vec<String>,
    /// Why no inventory could be read at all, if `progress.json` could not
    /// be parsed or did not describe one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl InventoryReport {
    /// Reconcile the pinned inventory against the receipts and destination
    /// rows that actually exist.
    ///
    /// The expectation is the union of three independent pins — the canonical
    /// sessions from `progress.json`, the variant sessions from the pinned
    /// mapping, and the delta-new sessions from the pinned delta inventory —
    /// so a receipt for a session none of them sanctions is still an error.
    /// The union never weakens reconciliation: it only lets each new receipt
    /// trace to the pin that sanctions it.
    fn reconcile(
        campaign: &Campaign,
        effective: &[Effective],
        destination: &BTreeSet<String>,
        variant_ids: &BTreeSet<String>,
        delta_ids: &BTreeSet<String>,
    ) -> Self {
        let receipts: BTreeSet<String> = effective
            .iter()
            .map(|entry| entry.session.clone())
            .collect();
        let counts = Self {
            receipts: receipts.len(),
            destination_rows: destination.len(),
            ..Self::default()
        };
        let inventory: Inventory = match campaign.inventory() {
            Ok(inventory) => inventory,
            Err(error) => {
                return Self {
                    error: Some(error.to_string()),
                    ..counts
                };
            }
        };

        let union: BTreeSet<String> = inventory
            .expected
            .union(variant_ids)
            .chain(delta_ids.iter())
            .cloned()
            .collect();
        let missing_receipts = difference(&union, &receipts);
        let unexpected_receipts = difference(&receipts, &union);
        let absent_destination = difference(&receipts, destination);
        let unexpected_destination = difference(destination, &receipts);
        let reconciled = missing_receipts.is_empty()
            && unexpected_receipts.is_empty()
            && absent_destination.is_empty()
            && unexpected_destination.is_empty()
            && inventory.deferred == 0
            && inventory.errors == 0
            && inventory.total == inventory.expected.len();

        Self {
            total: inventory.total,
            expected: inventory.expected.len(),
            deferred: inventory.deferred,
            errors: inventory.errors,
            variants_expected: variant_ids.len(),
            delta_expected: delta_ids.len(),
            progress_complete: inventory.complete,
            identity_digest: inventory.identity_digest,
            driver_digest: inventory.driver_digest,
            missing_receipts,
            unexpected_receipts,
            absent_destination,
            unexpected_destination,
            reconciled,
            error: None,
            ..counts
        }
    }
}

/// How a pass read the destination: which generation it saw, and whether
/// that generation still stood when it finished.
///
/// Kept together because the four facts only make sense as one claim — a
/// stable generation read inside a single transaction is what lets the
/// integrity checks and the digests be talked about in the same sentence.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SnapshotReport {
    /// Whether every read ran inside one SQLite read transaction, so the
    /// integrity checks and the digests describe the same snapshot.
    pub consistent_read: bool,
    /// `PRAGMA data_version` before the snapshot opened.
    pub data_version_before: i64,
    /// `PRAGMA data_version` after the snapshot committed.
    pub data_version_after: i64,
    /// Whether another connection committed to the destination while the
    /// pass ran. The pass itself stayed consistent — it read one snapshot —
    /// but the destination has since moved on, so re-run before relying on
    /// the result.
    pub concurrent_write: bool,
}

/// Everything one verification pass observed.
#[derive(Clone, Debug, Serialize)]
pub struct VerifyReport {
    /// Staging root that was verified.
    pub root: String,
    /// Sessions resolved from the journals.
    pub sessions_effective: usize,
    /// Sessions whose destination digest was recomputed.
    pub sessions_checked: usize,
    /// Sessions whose digest and message count matched their receipt.
    pub sessions_verified: usize,
    /// Whether this pass covered the whole finished campaign.
    ///
    /// True only when the inventory reconciled, the driver had finished, and
    /// every effective receipt was selected. A bounded pass leaves it false
    /// no matter how clean it was.
    pub complete: bool,
    /// How the pinned inventory reconciles against receipts and destination.
    pub inventory: InventoryReport,
    /// Result of `PRAGMA quick_check`.
    pub quick_check: String,
    /// Rows returned by `PRAGMA foreign_key_check`.
    pub foreign_key_violations: usize,
    /// How the destination was read, and which generation it saw.
    pub snapshot: SnapshotReport,
    /// Receipts that could not resolve to one effective receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chain_errors: Vec<String>,
    /// Selected sessions with no `session_v2` row in the destination.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Sessions whose destination digest differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub digest_mismatch: Vec<DigestMismatch>,
    /// Sessions that have been superseded before and whose destination
    /// digest differs from their effective receipt: changed again since the
    /// head receipt was written, so they need re-verification rather than
    /// being corrupt. A mismatch on a never-superseded session stays in
    /// `digest_mismatch` — there its base attestation itself is broken.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub superseded_unverified: Vec<DigestMismatch>,
    /// Sessions whose destination message count differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub message_count_mismatch: Vec<CountMismatch>,
    /// Sessions whose source snapshot digest differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_mismatch: Vec<String>,
    /// Sessions whose recovered-row digest differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recovery_mismatch: Vec<String>,
    /// Variant receipts whose re-key proof failed re-derivation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remap_mismatch: Vec<RemapMismatch>,
    /// Distinct `driverDigest` values found per journal. Reported, never
    /// compared in-band: the gate compares them against the pinned driver.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub driver_digests: BTreeMap<String, Vec<String>>,
    /// Sessions that could not be read from the source snapshots.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_errors: Vec<String>,
    /// Whether the pass was asked to recompute source and recovery digests.
    pub checked_sources: bool,
    /// Selected sessions whose source and recovery digests were both
    /// recomputed. Only meaningful when [`VerifyReport::checked_sources`].
    pub source_coverage: usize,
    /// Selected sessions `--sources` skipped for want of receipt metadata.
    ///
    /// Receipts are validated before a pass reads them, so this should stay
    /// empty; it exists so that a future schema change cannot quietly turn
    /// "no metadata" into "not checked" while coverage still reads clean.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources_uncovered: Vec<String>,
    /// Wall-clock milliseconds spent.
    pub elapsed_ms: u64,
}

impl VerifyReport {
    /// An empty report for `campaign`, recording what the pass was asked to
    /// cover before it has observed anything.
    fn new(campaign: &Campaign, options: &VerifyOptions) -> Self {
        Self {
            root: campaign.root().display().to_string(),
            sessions_effective: 0,
            sessions_checked: 0,
            sessions_verified: 0,
            complete: false,
            inventory: InventoryReport::default(),
            quick_check: String::new(),
            foreign_key_violations: 0,
            snapshot: SnapshotReport::default(),
            chain_errors: Vec::new(),
            missing: Vec::new(),
            digest_mismatch: Vec::new(),
            superseded_unverified: Vec::new(),
            message_count_mismatch: Vec::new(),
            source_mismatch: Vec::new(),
            recovery_mismatch: Vec::new(),
            remap_mismatch: Vec::new(),
            driver_digests: BTreeMap::new(),
            source_errors: Vec::new(),
            checked_sources: options.sources,
            source_coverage: 0,
            sources_uncovered: Vec::new(),
            elapsed_ms: 0,
        }
    }

    /// Whether the pass found no failure of any kind.
    ///
    /// Inventory reconciliation and source coverage count as failures: a
    /// pass that could not say what it was measuring, or that skipped part
    /// of what it was asked to check, has not verified anything.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.quick_check == "ok"
            && self.foreign_key_violations == 0
            && self.inventory.reconciled
            && self.chain_errors.is_empty()
            && self.missing.is_empty()
            && self.digest_mismatch.is_empty()
            && self.superseded_unverified.is_empty()
            && self.message_count_mismatch.is_empty()
            && self.source_mismatch.is_empty()
            && self.recovery_mismatch.is_empty()
            && self.remap_mismatch.is_empty()
            && self.source_errors.is_empty()
            && self.sources_uncovered.is_empty()
            && (!self.checked_sources || self.source_coverage == self.sessions_checked)
    }

    /// Whether the pass covers the whole campaign and found nothing wrong.
    ///
    /// A bounded pass can never satisfy this on its own; `--allow-partial`
    /// lets an operator accept one deliberately.
    #[must_use]
    pub fn succeeded(&self, allow_partial: bool) -> bool {
        self.clean() && (self.complete || allow_partial)
    }
}

/// A verification pass could not be completed.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The campaign could not be read.
    #[error(transparent)]
    Campaign(#[from] CampaignError),
    /// A database could not be opened.
    #[error("cannot open {path}: {source}")]
    Open {
        /// Database that could not be opened.
        path: PathBuf,
        /// Underlying SQLite failure.
        source: rusqlite::Error,
    },
    /// An already-open database could not be read from.
    #[error("cannot read {path}: {source}")]
    Read {
        /// Database that could not be read.
        path: PathBuf,
        /// Underlying SQLite failure.
        source: rusqlite::Error,
    },
    /// A digest could not be recomputed.
    #[error("digest {kind} for {session}: {source}")]
    Digest {
        /// Which digest was being computed.
        kind: String,
        /// Session it was computed for.
        session: String,
        /// Underlying failure.
        source: DigestError,
    },
    /// A requested session is not in the campaign journals.
    #[error("session {0} has no receipt in this campaign")]
    UnknownSession(String),
    /// The variant identity or pinned mapping could not be verified. A pass
    /// that continued would check variant receipts against an unattested
    /// mapping, which is exactly the "verify nothing" path the inventory
    /// rules close everywhere else.
    #[error("variant mapping: {reason}")]
    Mapping {
        /// What disagreed or could not be read.
        reason: String,
    },
}

/// Reopen a database read-only, so verification can never perturb a campaign.
fn read_only(path: &std::path::Path) -> Result<Connection, VerifyError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|source| {
        VerifyError::Open {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// `PRAGMA data_version`, which SQLite bumps only when some other
/// connection commits a change to the database.
fn data_version(connection: &Connection) -> Result<i64, rusqlite::Error> {
    connection
        .prepare("PRAGMA data_version")?
        .query_row([], |row| row.get::<_, i64>(0))
}

/// One database held inside a read transaction, so every read a pass makes
/// through it belongs to a single SQLite snapshot.
///
/// Without this the pass reads whatever each query happens to see:
/// `quick_check` could walk one version of the file while the digest loop
/// hashes another, and nothing in the report would say so. Dropping the
/// value rolls the transaction back; [`Snapshot::finish`] commits it and
/// reports whether anybody else wrote in the meantime.
#[derive(Debug)]
struct Snapshot {
    /// Connection the transaction was opened on.
    connection: Connection,
    /// Database the transaction covers, for error messages.
    path: PathBuf,
    /// `data_version` as it was before the transaction started.
    version_before: i64,
}

impl Snapshot {
    /// Open `path` read-only and start a read transaction on it.
    ///
    /// `BEGIN` is deferred: SQLite takes no snapshot until the transaction
    /// reads something, which would leave a window between opening the
    /// snapshot and the first check. One read at open time closes it, so the
    /// generation reported alongside the pass is the generation the pass
    /// actually reads.
    fn open(path: &std::path::Path) -> Result<Self, VerifyError> {
        let connection = read_only(path)?;
        let version_before = data_version(&connection).map_err(|source| VerifyError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        connection
            .execute_batch("BEGIN")
            .map_err(|source| VerifyError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        connection
            .prepare("SELECT count(*) FROM sqlite_master")
            .and_then(|mut statement| statement.query_row([], |row| row.get::<_, i64>(0)))
            .map_err(|source| VerifyError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            version_before,
        })
    }

    /// The connection every read should go through.
    fn connection(&self) -> &Connection {
        &self.connection
    }

    /// End the transaction and report `data_version` as it stands
    /// afterwards, so the caller can tell whether anybody wrote while the
    /// pass was running.
    fn finish(self) -> Result<i64, VerifyError> {
        let path = self.path.clone();
        self.connection
            .execute_batch("COMMIT")
            .map_err(|source| VerifyError::Read { path, source })?;
        data_version(&self.connection).map_err(|source| VerifyError::Read {
            path: self.path.clone(),
            source,
        })
    }
}

/// `PRAGMA quick_check`, which walks the whole database structure.
fn quick_check(connection: &Connection) -> Result<String, rusqlite::Error> {
    connection
        .prepare("PRAGMA quick_check")?
        .query_row([], |row| row.get::<_, String>(0))
}

/// Count of `PRAGMA foreign_key_check` rows, zero when the database is sound.
fn foreign_key_violations(connection: &Connection) -> Result<usize, rusqlite::Error> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    let mut violations = 0_usize;
    while rows.next()?.is_some() {
        violations += 1;
    }
    Ok(violations)
}

/// Every session id the destination holds, so inventory reconciliation does
/// not depend on which sessions this pass selected.
fn destination_sessions(connection: &Connection) -> Result<BTreeSet<String>, rusqlite::Error> {
    let mut statement = connection.prepare("SELECT id FROM session_v2 ORDER BY id")?;
    let mut rows = statement.query([])?;
    let mut found = BTreeSet::new();
    while let Some(row) = rows.next()? {
        found.insert(row.get::<_, String>(0)?);
    }
    Ok(found)
}

/// Elements of `left` that `right` does not hold.
fn difference(left: &BTreeSet<String>, right: &BTreeSet<String>) -> Vec<String> {
    left.difference(right).cloned().collect()
}

/// Run one read-only verification pass over a campaign.
///
/// # Errors
///
/// Returns [`VerifyError::Campaign`] when the journals cannot be read or do
/// not resolve, [`VerifyError::UnknownSession`] when `--session` names a
/// receipt that does not exist, [`VerifyError::Open`] when a database cannot
/// be opened, [`VerifyError::Read`] when an open database cannot be queried
/// or its snapshot cannot be held, and [`VerifyError::Digest`] when a digest
/// cannot be recomputed. Inventory and chain *content* problems are not
/// errors: they are outcomes the report records, so an operator still gets
/// the integrity results for a campaign that does not reconcile.
pub fn verify(campaign: &Campaign, options: &VerifyOptions) -> Result<VerifyReport, VerifyError> {
    let started = Instant::now();
    let mut report = VerifyReport::new(campaign, options);

    let effective = match campaign.effective_receipts() {
        Ok(effective) => effective,
        Err(error) => {
            report.chain_errors.push(error.to_string());
            Vec::new()
        }
    };
    report.sessions_effective = effective.len();

    let selected = select(&effective, options)?;

    let destination = Snapshot::open(&campaign.destination())?;
    report.snapshot.consistent_read = true;
    report.snapshot.data_version_before = destination.version_before;
    let path = campaign.destination();
    report.quick_check =
        quick_check(destination.connection()).map_err(|source| VerifyError::Read {
            path: path.clone(),
            source,
        })?;
    report.foreign_key_violations =
        foreign_key_violations(destination.connection()).map_err(|source| VerifyError::Read {
            path: path.clone(),
            source,
        })?;
    let rows =
        destination_sessions(destination.connection()).map_err(|source| VerifyError::Read {
            path: path.clone(),
            source,
        })?;

    report.inventory = InventoryReport::reconcile(campaign, &effective, &rows);
    report.complete = report.inventory.reconciled
        && report.inventory.progress_complete
        && selected.len() == effective.len();

    let mut sources: HashMap<String, Snapshot> = HashMap::new();
    let recovery = if options.sources {
        Some(Snapshot::open(&campaign.recovery())?)
    } else {
        None
    };

    for entry in &selected {
        report.sessions_checked += 1;
        let receipt = &entry.head;
        let expected = receipt.destination_digest().unwrap_or_default().to_string();

        let actual = session_digest(destination.connection(), &entry.session);

        match actual {
            Ok(actual) => {
                let actual_messages = i64::try_from(actual.messages).unwrap_or(i64::MAX);
                let digest_matches = actual.digest == expected;
                if !digest_matches {
                    let mismatch = DigestMismatch {
                        session: entry.session.clone(),
                        expected,
                        actual: actual.digest,
                    };
                    if entry.depth > 1 {
                        report.superseded_unverified.push(mismatch);
                    } else {
                        report.digest_mismatch.push(mismatch);
                    }
                }
                let count_matches = receipt.messages() == Some(actual_messages);
                if !count_matches {
                    report.message_count_mismatch.push(CountMismatch {
                        session: entry.session.clone(),
                        expected: receipt.messages().unwrap_or_default(),
                        actual: actual_messages,
                    });
                }
                if digest_matches && count_matches {
                    report.sessions_verified += 1;
                }
            }
            Err(DigestError::MissingSession(_)) => report.missing.push(entry.session.clone()),
            Err(source) => {
                return Err(VerifyError::Digest {
                    kind: "destination".to_string(),
                    session: entry.session.clone(),
                    source,
                });
            }
        }

        if options.sources {
            check_sources(
                campaign,
                entry,
                &mut sources,
                recovery.as_ref(),
                &mut report,
            )?;
        }
    }

    // Only the destination's snapshot is reported: the source snapshots
    // exist so each recomputation reads one version of its own database,
    // and they end when this call returns.
    let version_after = destination.finish()?;
    report.snapshot.data_version_after = version_after;
    report.snapshot.concurrent_write = version_after != report.snapshot.data_version_before;
    report.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(report)
}

/// Resolve which receipts this pass covers, honouring explicit `--session`
/// filters before applying the bound.
fn select(effective: &[Effective], options: &VerifyOptions) -> Result<Vec<Effective>, VerifyError> {
    if options.sessions.is_empty() {
        let selected: Vec<Effective> = effective
            .iter()
            .take(if options.limit == 0 {
                effective.len()
            } else {
                options.limit
            })
            .cloned()
            .collect();
        return Ok(selected);
    }

    let wanted: BTreeSet<&str> = options.sessions.iter().map(String::as_str).collect();
    let mut selected = Vec::new();
    for entry in effective {
        if wanted.contains(entry.session.as_str()) {
            selected.push(entry.clone());
        }
    }
    if selected.len() != wanted.len() {
        let known: BTreeSet<&str> = effective
            .iter()
            .map(|entry| entry.session.as_str())
            .collect();
        for session in &wanted {
            if !known.contains(session) {
                return Err(VerifyError::UnknownSession((*session).to_string()));
            }
        }
    }
    selected.sort_by(|left, right| left.session.cmp(&right.session));
    Ok(selected)
}

/// Recompute `inputDigest` and `recoveryDigest` against the frozen snapshots.
///
/// Both snapshots are read inside their own transactions, so a receipt's two
/// digests are recomputed against one version of each source rather than
/// against a file that is being rewritten underneath the pass.
fn check_sources(
    campaign: &Campaign,
    entry: &Effective,
    sources: &mut HashMap<String, Snapshot>,
    recovery: Option<&Snapshot>,
    report: &mut VerifyReport,
) -> Result<(), VerifyError> {
    let receipt = &entry.head;
    // Receipts are validated before verification reads them, so all three
    // fields are always present. The guard stays because "absent" has to
    // mean "not covered", never "covered and quietly skipped".
    let (Some(source), Some(input), Some(expected_recovery)) = (
        receipt.source(),
        receipt.input_digest(),
        receipt.recovery_digest(),
    ) else {
        report.sources_uncovered.push(entry.session.clone());
        return Ok(());
    };

    if !sources.contains_key(source) {
        let snapshot = Snapshot::open(&campaign.source(source))?;
        sources.insert(source.to_string(), snapshot);
    }
    // Variant receipts attest to a derived session, but their source
    // digests were computed against the session they derive from.
    let source_session = receipt.source_session_id().unwrap_or(&entry.session);
    let snapshot = &sources[source];
    match session_digest(snapshot.connection(), source_session) {
        Ok(actual) => {
            if Some(actual.digest.as_str()) != Some(input) {
                report.source_mismatch.push(entry.session.clone());
            }
        }
        Err(DigestError::MissingSession(_)) => report.source_errors.push(entry.session.clone()),
        Err(source_error) => {
            return Err(VerifyError::Digest {
                kind: "source".to_string(),
                session: entry.session.clone(),
                source: source_error,
            });
        }
    }

    if let Some(recovery) = recovery {
        match recovered_hash(recovery.connection(), source_session) {
            Ok(actual) => {
                if actual != expected_recovery {
                    report.recovery_mismatch.push(entry.session.clone());
                }
            }
            Err(source_error) => {
                return Err(VerifyError::Digest {
                    kind: "recovery".to_string(),
                    session: entry.session.clone(),
                    source: source_error,
                });
            }
        }
    }

    report.source_coverage += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a fixture destination, putting it in WAL first: a read
    /// transaction in rollback-journal mode holds a SHARED lock that would
    /// make the writer below block instead of committing underneath us.
    fn destination_file() -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        let connection = Connection::open(file.path()).expect("connection");
        let mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .expect("wal");
        assert_eq!(mode.to_lowercase(), "wal");
        connection
            .execute_batch(
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY);
                 INSERT INTO session_v2 VALUES ('ses_fixture01');",
            )
            .expect("schema");
        drop(connection);
        file
    }

    /// The whole point of holding a transaction: a commit that lands while
    /// the pass is running stays invisible, so `quick_check` and the digest
    /// loop cannot end up describing two different versions of the file.
    #[test]
    fn a_snapshot_does_not_see_a_write_committed_while_it_is_open() {
        let file = destination_file();
        let snapshot = Snapshot::open(file.path()).expect("snapshot");
        let version_before = snapshot.version_before;

        let writer = Connection::open(file.path()).expect("writer");
        writer
            .execute("INSERT INTO session_v2 VALUES ('ses_late')", [])
            .expect("commit");
        drop(writer);

        let rows = destination_sessions(snapshot.connection()).expect("reads");
        assert_eq!(
            rows.len(),
            1,
            "the open snapshot still describes the state it started with"
        );

        let version_after = snapshot.finish().expect("commits");
        assert_ne!(
            version_after, version_before,
            "the generation moved while the pass ran"
        );

        let reopened = Connection::open(file.path()).expect("reopen");
        let rows = destination_sessions(&reopened).expect("reads");
        assert_eq!(
            rows.len(),
            2,
            "once the transaction ends the write is visible"
        );
    }

    /// A pass over an untouched database reports a stable generation, which
    /// is what makes `concurrent_write` mean something when it does fire.
    #[test]
    fn a_quiet_pass_reports_a_stable_generation() {
        let file = destination_file();
        let snapshot = Snapshot::open(file.path()).expect("snapshot");
        let version_before = snapshot.version_before;
        let rows = destination_sessions(snapshot.connection()).expect("reads");
        assert_eq!(rows.len(), 1);

        let version_after = snapshot.finish().expect("commits");
        assert_eq!(
            version_after, version_before,
            "nobody wrote during the pass"
        );
    }

    /// Dropping a snapshot without finishing it rolls back rather than
    /// leaving a transaction open on a connection nobody holds.
    #[test]
    fn a_snapshot_that_is_dropped_leaves_no_transaction_behind() {
        let file = destination_file();
        let version_before = {
            let snapshot = Snapshot::open(file.path()).expect("snapshot");
            let version = snapshot.version_before;
            assert_eq!(
                destination_sessions(snapshot.connection())
                    .expect("rows")
                    .len(),
                1
            );
            version
        };

        let reopened = Connection::open(file.path()).expect("reopen");
        let version_after = data_version(&reopened).expect("data_version");
        assert_eq!(version_after, version_before, "nothing was written");
        assert_eq!(
            destination_sessions(&reopened).expect("rows").len(),
            1,
            "the table still holds what it did"
        );
    }
}
