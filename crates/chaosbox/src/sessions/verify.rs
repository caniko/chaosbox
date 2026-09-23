//! Read-only verification of a campaign against its destination database.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    time::Instant,
};

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::sessions::{
    campaign::{Campaign, CampaignError, Effective},
    digest::{recovered_hash, session_digest, DigestError},
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
    /// Whether every effective receipt was checked.
    pub complete: bool,
    /// Result of `PRAGMA quick_check`.
    pub quick_check: String,
    /// Rows returned by `PRAGMA foreign_key_check`.
    pub foreign_key_violations: usize,
    /// Receipts that could not resolve to one effective receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chain_errors: Vec<String>,
    /// Sessions with no `session_v2` row in the destination.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Sessions whose destination digest differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub digest_mismatch: Vec<DigestMismatch>,
    /// Sessions whose destination message count differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub message_count_mismatch: Vec<CountMismatch>,
    /// Sessions whose source snapshot digest differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_mismatch: Vec<String>,
    /// Sessions whose recovered-row digest differs from the receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recovery_mismatch: Vec<String>,
    /// Sessions that could not be read from the source snapshots.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_errors: Vec<String>,
    /// Whether source and recovery digests were checked too.
    pub checked_sources: bool,
    /// Wall-clock milliseconds spent.
    pub elapsed_ms: u64,
}

impl VerifyReport {
    /// Whether the pass found no failure of any kind.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.quick_check == "ok"
            && self.foreign_key_violations == 0
            && self.chain_errors.is_empty()
            && self.missing.is_empty()
            && self.digest_mismatch.is_empty()
            && self.message_count_mismatch.is_empty()
            && self.source_mismatch.is_empty()
            && self.recovery_mismatch.is_empty()
            && self.source_errors.is_empty()
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

/// Run one read-only verification pass over a campaign.
///
/// # Errors
///
/// Returns [`VerifyError::Campaign`] when the journals cannot be read or do
/// not resolve, [`VerifyError::UnknownSession`] when `--session` names a
/// receipt that does not exist, [`VerifyError::Open`] when a database cannot
/// be opened, and [`VerifyError::Digest`] when a digest cannot be recomputed.
pub fn verify(campaign: &Campaign, options: &VerifyOptions) -> Result<VerifyReport, VerifyError> {
    let started = Instant::now();
    let mut report = VerifyReport {
        root: campaign.root().display().to_string(),
        sessions_effective: 0,
        sessions_checked: 0,
        sessions_verified: 0,
        complete: false,
        quick_check: String::new(),
        foreign_key_violations: 0,
        chain_errors: Vec::new(),
        missing: Vec::new(),
        digest_mismatch: Vec::new(),
        message_count_mismatch: Vec::new(),
        source_mismatch: Vec::new(),
        recovery_mismatch: Vec::new(),
        source_errors: Vec::new(),
        checked_sources: options.sources,
        elapsed_ms: 0,
    };

    let effective = match campaign.effective_receipts() {
        Ok(effective) => effective,
        Err(error) => {
            report.chain_errors.push(error.to_string());
            Vec::new()
        }
    };
    report.sessions_effective = effective.len();

    let selected = select(&effective, options)?;
    report.complete = selected.len() == effective.len();

    let destination = read_only(&campaign.destination())?;
    report.quick_check = quick_check(&destination).map_err(|source| VerifyError::Open {
        path: campaign.destination(),
        source,
    })?;
    report.foreign_key_violations =
        foreign_key_violations(&destination).map_err(|source| VerifyError::Open {
            path: campaign.destination(),
            source,
        })?;

    let mut sources: HashMap<String, Connection> = HashMap::new();
    let recovery = if options.sources {
        Some(read_only(&campaign.recovery())?)
    } else {
        None
    };

    for entry in &selected {
        report.sessions_checked += 1;
        let receipt = &entry.head;
        let expected = receipt.destination_digest().unwrap_or_default().to_string();

        let actual = session_digest(&destination, &entry.session);

        match actual {
            Ok(actual) => {
                let actual_messages = i64::try_from(actual.messages).unwrap_or(i64::MAX);
                let digest_matches = actual.digest == expected;
                if !digest_matches {
                    report.digest_mismatch.push(DigestMismatch {
                        session: entry.session.clone(),
                        expected,
                        actual: actual.digest,
                    });
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
fn check_sources(
    campaign: &Campaign,
    entry: &Effective,
    sources: &mut HashMap<String, Connection>,
    recovery: Option<&Connection>,
    report: &mut VerifyReport,
) -> Result<(), VerifyError> {
    let receipt = &entry.head;
    if let Some(source) = receipt.source() {
        if !sources.contains_key(source) {
            let connection = read_only(&campaign.source(source))?;
            sources.insert(source.to_string(), connection);
        }
        let connection = &sources[source];
        match session_digest(connection, &entry.session) {
            Ok(actual) => {
                if Some(actual.digest.as_str()) != receipt.input_digest() {
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
    }

    if let Some(recovery) = recovery {
        match recovered_hash(recovery, &entry.session) {
            Ok(actual) => {
                if Some(actual.as_str()) != receipt.recovery_digest() {
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
    Ok(())
}
