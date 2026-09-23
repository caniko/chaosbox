//! Campaign discovery, progress, and supersession-aware receipt resolution.
//!
//! A campaign staging root holds `progress.json`, a `destination.db`, and one
//! or more `journal-*` directories of receipts. Base receipts live in
//! `journal-v2`; a changed session gets an additional record in `journal-v3`
//! whose `supersedes` field names the receipt it replaces. Receipts are
//! immutable, so supersession is always forward: an old receipt keeps its
//! bytes and a newer one points back at it. Exactly one receipt per session
//! is unreferenced, and that one is the effective receipt `verify` checks.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::sessions::inventory::Inventory;

/// Bytes a source snapshot name may use, so a receipt cannot steer
/// [`Campaign::source`] outside the campaign's `work/` directory. Real
/// names are `primary`, `stable`, `old-backup`, `local`, `quarantine`, and
/// `canary`; a path separator or a `..` component is never one of them.
const SOURCE_BYTES: [u8; 2] = *b"_-";

/// A receipt file discovered in a campaign journal.
#[derive(Clone, Debug)]
pub struct Receipt {
    /// Journal directory holding the file, such as `journal-v2`.
    pub journal: String,
    /// File name inside the journal, such as `ses_abc.json`.
    pub file: String,
    /// Session the receipt attests to.
    pub session: String,
    /// Parsed receipt body exactly as written.
    pub body: Value,
}

impl Receipt {
    /// Stable `journal/file` address another receipt can name.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}", self.journal, self.file)
    }

    /// Confirm this receipt satisfies the receipt contract.
    ///
    /// Every receipt attests to the same core facts, whether it is the base
    /// record of a session or a supersession that replaces an earlier one.
    /// The only field that distinguishes the two is `supersedes`, so it has
    /// to be unambiguous: a `supersedes` of the wrong type is an error
    /// rather than a receipt quietly read as a base record with a fresh
    /// set of attestation fields.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::MissingField`] when a required field is
    /// absent, and [`CampaignError::InvalidField`] when one is present but
    /// has the wrong type or shape.
    pub fn validate(&self) -> Result<(), CampaignError> {
        let key = self.key();
        text(&self.body, "sessionID", &key)?;
        let source = text(&self.body, "source", &key)?;
        if !is_source_name(source) {
            return Err(invalid_field(
                &key,
                "source",
                "expected a bare database name, without path separators",
            ));
        }
        for field in ["inputDigest", "recoveryDigest", "destinationDigest"] {
            digest(&self.body, field, &key)?;
        }
        count(&self.body, "messages", &key)?;
        if self.body.get("drafts").is_some() {
            count(&self.body, "drafts", &key)?;
        }
        if let Some(value) = self.body.get("transformation") {
            if value.as_str().is_none() {
                return Err(invalid_field(&key, "transformation", "expected a string"));
            }
        }
        if let Some(value) = self.body.get("version") {
            match value.as_i64() {
                Some(version) if version > 0 => {}
                _ => {
                    return Err(invalid_field(
                        &key,
                        "version",
                        "expected a positive integer",
                    ));
                }
            }
        }
        if let Some(value) = self.body.get("supersedes") {
            let target = value
                .as_str()
                .ok_or_else(|| invalid_field(&key, "supersedes", "expected a journal address"))?;
            if !is_journal_address(target) {
                return Err(invalid_field(
                    &key,
                    "supersedes",
                    "expected a `journal-*/<receipt>.json` address",
                ));
            }
        }
        Ok(())
    }

    /// Session this receipt attests to.
    ///
    /// [`Receipt::validate`] runs before any receipt is handed out, so the
    /// field is always present and always a string.
    #[must_use]
    pub fn session_id(&self) -> &str {
        self.body
            .get("sessionID")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// Journal address this receipt replaces, if it is a supersession record.
    ///
    /// [`Receipt::validate`] runs before this is consulted, so a
    /// `supersedes` of the wrong type has already been refused rather than
    /// being read here as an absent field.
    #[must_use]
    pub fn supersedes(&self) -> Option<&str> {
        self.body.get("supersedes").and_then(Value::as_str)
    }

    /// Digest the destination must reproduce for this session.
    #[must_use]
    pub fn destination_digest(&self) -> Option<&str> {
        self.body.get("destinationDigest").and_then(Value::as_str)
    }

    /// Digest of the source snapshot the session was recovered from.
    #[must_use]
    pub fn input_digest(&self) -> Option<&str> {
        self.body.get("inputDigest").and_then(Value::as_str)
    }

    /// Digest of the recovered rows backing this session.
    #[must_use]
    pub fn recovery_digest(&self) -> Option<&str> {
        self.body.get("recoveryDigest").and_then(Value::as_str)
    }

    /// Source database the input digest covers, such as `primary`.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.body.get("source").and_then(Value::as_str)
    }

    /// Message count attested when the receipt was written.
    #[must_use]
    pub fn messages(&self) -> Option<i64> {
        self.body.get("messages").and_then(Value::as_i64)
    }
}

/// The single receipt a session currently resolves to, and how deep its
/// supersession chain ran.
#[derive(Clone, Debug)]
pub struct Effective {
    /// Session the chain belongs to.
    pub session: String,
    /// Head of the chain: nothing supersedes this receipt.
    pub head: Receipt,
    /// Receipts in the chain, including the head.
    pub depth: usize,
}

/// An identity document pinned by one journal.
#[derive(Clone, Debug)]
pub struct Identity {
    /// Journal directory holding the document.
    pub journal: String,
    /// File name, such as `identity.json`.
    pub file: String,
    /// Parsed document exactly as written.
    pub body: Value,
}

/// A campaign staging root could not be read or its receipts could not be
/// resolved to exactly one effective receipt each.
#[derive(Debug, thiserror::Error)]
pub enum CampaignError {
    /// Neither `--root` nor `CHAOSBOX_SESSION_CAMPAIGN` named a campaign.
    #[error("no campaign root: pass --root or set CHAOSBOX_SESSION_CAMPAIGN")]
    MissingRoot,
    /// The directory lacks the layout of a staging root.
    #[error("not a campaign staging root: {path} is missing")]
    MissingLayout {
        /// Path that was absent.
        path: PathBuf,
    },
    /// A file in the campaign could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O failure.
        source: std::io::Error,
    },
    /// A JSON file in the campaign could not be parsed.
    #[error("cannot parse {path}: {source}")]
    Parse {
        /// File that could not be parsed.
        path: PathBuf,
        /// Underlying JSON failure.
        source: serde_json::Error,
    },
    /// A supersession named a receipt that does not exist.
    #[error("receipt {key} supersedes missing receipt {target}")]
    DanglingSupersession {
        /// Receipt that named a missing target.
        key: String,
        /// Journal address that was not found.
        target: String,
    },
    /// A supersession named a receipt belonging to a different session.
    #[error("receipt {key} supersedes {target} of another session")]
    ForeignSupersession {
        /// Receipt that crossed a session boundary.
        key: String,
        /// Receipt it wrongly claimed to replace.
        target: String,
    },
    /// More than one receipt in the chain is unreferenced, so no single
    /// receipt is authoritative.
    #[error("session {session} has {count} receipts that nothing supersedes; one effective receipt is required")]
    AmbiguousEffective {
        /// Session with competing receipts.
        session: String,
        /// How many receipts nothing supersedes.
        count: usize,
    },
    /// Every receipt in the chain is referenced by another: it cycles.
    #[error("session {session} supersession chain cycles")]
    CyclicChain {
        /// Session whose chain has no head.
        session: String,
    },
    /// Walking the chain from its head did not reach every receipt.
    #[error("session {session} chain reaches {visited} of {expected} receipts")]
    DisconnectedChain {
        /// Session whose chain has a detached component.
        session: String,
        /// Receipts the walk visited.
        visited: usize,
        /// Receipts that exist for the session.
        expected: usize,
    },
    /// A receipt inside a chain claims a different session than its filename.
    #[error("receipt {key} attests to {found} instead of {expected}")]
    SessionMismatch {
        /// Receipt whose body disagreed with its filename.
        key: String,
        /// Session the filename promises.
        expected: String,
        /// Session the body claims.
        found: String,
    },
    /// A required receipt field was not written at all.
    #[error("receipt {key} has no {field}")]
    MissingField {
        /// Receipt that was incomplete.
        key: String,
        /// Field that was absent.
        field: String,
    },
    /// A receipt field was written with the wrong type or shape.
    #[error("receipt {key} field {field} is invalid: {problem}")]
    InvalidField {
        /// Receipt that was malformed.
        key: String,
        /// Field that was malformed.
        field: String,
        /// What the contract expected instead.
        problem: String,
    },
    /// `progress.json` does not describe an inventory a pass can be
    /// measured against.
    #[error("progress.json field {field} is invalid: {problem}")]
    InvalidProgress {
        /// Field that was malformed.
        field: String,
        /// What the contract expected instead.
        problem: String,
    },
}

/// A campaign staging root opened read-only.
#[derive(Clone, Debug)]
pub struct Campaign {
    root: PathBuf,
}

impl Campaign {
    /// Open a staging root from an explicit path or the environment.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::MissingRoot`] when neither source names a
    /// campaign, and [`CampaignError::MissingLayout`] when the directory does
    /// not hold `progress.json` and `destination.db`.
    pub fn open(root: Option<PathBuf>) -> Result<Self, CampaignError> {
        let root = match root {
            Some(path) => path,
            None => env::var("CHAOSBOX_SESSION_CAMPAIGN")
                .map(PathBuf::from)
                .map_err(|_| CampaignError::MissingRoot)?,
        };
        for required in ["progress.json", "destination.db"] {
            let path = root.join(required);
            if !path.is_file() {
                return Err(CampaignError::MissingLayout { path });
            }
        }
        Ok(Self { root })
    }

    /// Staging root the campaign was opened from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of the destination database receipts attest to.
    #[must_use]
    pub fn destination(&self) -> PathBuf {
        self.root.join("destination.db")
    }

    /// Absolute path of the recovered-rows snapshot.
    #[must_use]
    pub fn recovery(&self) -> PathBuf {
        self.root
            .parent()
            .unwrap_or(&self.root)
            .join("recovered-rows.db")
    }

    /// Absolute path of one source snapshot, given its `source` name.
    #[must_use]
    pub fn source(&self, source: &str) -> PathBuf {
        self.root
            .parent()
            .unwrap_or(&self.root)
            .join("work")
            .join(format!("{source}.db"))
    }

    /// Read `progress.json`, the run counters the driver writes.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::Read`] or [`CampaignError::Parse`] when the
    /// file is unreadable or malformed.
    pub fn progress(&self) -> Result<Value, CampaignError> {
        Self::read_json(&self.root.join("progress.json"))
    }

    /// Read the pinned session inventory, the expectation a pass measures
    /// the journals and the destination against.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::Read`] or [`CampaignError::Parse`] when
    /// `progress.json` is unreadable or malformed, and
    /// [`CampaignError::InvalidProgress`] when it does not describe an
    /// inventory.
    pub fn inventory(&self) -> Result<Inventory, CampaignError> {
        Inventory::from_progress(&self.progress()?)
    }

    /// Read every identity document the journals pin, oldest journal first.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::Read`] or [`CampaignError::Parse`] when a
    /// document is unreadable or malformed.
    pub fn identities(&self) -> Result<Vec<Identity>, CampaignError> {
        let mut found = Vec::new();
        for journal in self.journals()? {
            for file in self.json_files(&journal)? {
                if !file.starts_with("identity") {
                    continue;
                }
                found.push(Identity {
                    journal: journal.clone(),
                    file: file.clone(),
                    body: Self::read_json(&self.root.join(&journal).join(&file))?,
                });
            }
        }
        Ok(found)
    }

    /// List every receipt across all journals, sorted by session then key.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::Read`] or [`CampaignError::Parse`] when a
    /// receipt is unreadable or malformed.
    pub fn receipts(&self) -> Result<Vec<Receipt>, CampaignError> {
        let mut receipts = Vec::new();
        for journal in self.journals()? {
            for file in self.json_files(&journal)? {
                let Some(session) = session_of_receipt(&file) else {
                    continue;
                };
                let path = self.root.join(&journal).join(&file);
                let body = Self::read_json(&path)?;
                let receipt = Receipt {
                    journal: journal.clone(),
                    file: file.clone(),
                    session,
                    body,
                };
                receipt.validate()?;
                receipts.push(receipt);
            }
        }
        receipts.sort_by(|left, right| {
            left.session
                .cmp(&right.session)
                .then_with(|| left.key().cmp(&right.key()))
        });
        Ok(receipts)
    }

    /// Resolve every session to its one effective receipt.
    ///
    /// # Errors
    ///
    /// Returns a chain error when the receipts of a session do not form a
    /// single acyclic chain ending in exactly one head, plus the read and
    /// parse errors from [`Campaign::receipts`].
    pub fn effective_receipts(&self) -> Result<Vec<Effective>, CampaignError> {
        let receipts = self.receipts()?;
        let by_key: BTreeMap<String, usize> = receipts
            .iter()
            .enumerate()
            .map(|(index, receipt)| (receipt.key(), index))
            .collect();
        let grouped = group_by_session(&receipts);
        let incoming = incoming_counts(&receipts, &by_key)?;

        let mut effective = Vec::with_capacity(grouped.len());
        for (session, members) in grouped {
            check_session_ids(&receipts, &members, &session)?;
            let head_index = single_head(&members, &incoming, &session)?;
            let depth = walk_chain(&receipts, &by_key, head_index, members.len(), &session)?;
            effective.push(Effective {
                session,
                head: receipts[head_index].clone(),
                depth,
            });
        }
        Ok(effective)
    }

    /// Journal directories under the root, sorted for determinism.
    fn journals(&self) -> Result<Vec<String>, CampaignError> {
        let entries = fs::read_dir(&self.root).map_err(|source| CampaignError::Read {
            path: self.root.clone(),
            source,
        })?;
        let mut journals: BTreeSet<String> = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|source| CampaignError::Read {
                path: self.root.clone(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("journal-") && entry.path().is_dir() {
                journals.insert(name);
            }
        }
        Ok(journals.into_iter().collect())
    }

    /// JSON file names inside one journal, sorted.
    fn json_files(&self, journal: &str) -> Result<Vec<String>, CampaignError> {
        let directory = self.root.join(journal);
        let entries = fs::read_dir(&directory).map_err(|source| CampaignError::Read {
            path: directory.clone(),
            source,
        })?;
        let mut files: BTreeSet<String> = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|source| CampaignError::Read {
                path: directory.clone(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
                files.insert(name);
            }
        }
        Ok(files.into_iter().collect())
    }

    /// Parse one JSON file, attributing failures to their path.
    fn read_json(path: &Path) -> Result<Value, CampaignError> {
        let text = fs::read_to_string(path).map_err(|source| CampaignError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| CampaignError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Group receipt indices by the session each one attests to, sorted.
fn group_by_session(receipts: &[Receipt]) -> BTreeMap<String, Vec<usize>> {
    let mut grouped: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, receipt) in receipts.iter().enumerate() {
        grouped
            .entry(receipt.session.clone())
            .or_default()
            .push(index);
    }
    grouped
}

/// How many receipts name each index as the one it supersedes.
fn incoming_counts(
    receipts: &[Receipt],
    by_key: &BTreeMap<String, usize>,
) -> Result<Vec<usize>, CampaignError> {
    let mut incoming = vec![0_usize; receipts.len()];
    for receipt in receipts {
        let Some(target) = receipt.supersedes() else {
            continue;
        };
        let key = receipt.key();
        let Some(&target_index) = by_key.get(target) else {
            return Err(CampaignError::DanglingSupersession {
                key,
                target: target.to_string(),
            });
        };
        if receipts[target_index].session != receipt.session {
            return Err(CampaignError::ForeignSupersession {
                key,
                target: target.to_string(),
            });
        }
        incoming[target_index] += 1;
    }
    Ok(incoming)
}

/// Confirm every receipt in a chain attests to the session its filename does.
fn check_session_ids(
    receipts: &[Receipt],
    members: &[usize],
    session: &str,
) -> Result<(), CampaignError> {
    for &index in members {
        let receipt = &receipts[index];
        if receipt.session_id() == session {
            continue;
        }
        return Err(CampaignError::SessionMismatch {
            key: receipt.key(),
            expected: session.to_string(),
            found: receipt.session_id().to_string(),
        });
    }
    Ok(())
}

/// The one receipt in a session that nothing supersedes.
fn single_head(
    members: &[usize],
    incoming: &[usize],
    session: &str,
) -> Result<usize, CampaignError> {
    let mut heads = members
        .iter()
        .copied()
        .filter(|index| incoming[*index] == 0);
    let head = heads.next();
    match (head, heads.next()) {
        (None, _) => Err(CampaignError::CyclicChain {
            session: session.to_string(),
        }),
        (Some(head), None) => Ok(head),
        (Some(_), Some(_)) => Err(CampaignError::AmbiguousEffective {
            session: session.to_string(),
            count: members
                .iter()
                .filter(|index| incoming[**index] == 0)
                .count(),
        }),
    }
}

/// Walk from the head down to the base receipt, returning the chain depth.
fn walk_chain(
    receipts: &[Receipt],
    by_key: &BTreeMap<String, usize>,
    head_index: usize,
    expected: usize,
    session: &str,
) -> Result<usize, CampaignError> {
    let mut visited = 1_usize;
    let mut current = head_index;
    while let Some(target) = receipts[current].supersedes() {
        let Some(&next) = by_key.get(target) else {
            return Err(CampaignError::DanglingSupersession {
                key: receipts[current].key(),
                target: target.to_string(),
            });
        };
        visited += 1;
        if visited > expected {
            return Err(CampaignError::CyclicChain {
                session: session.to_string(),
            });
        }
        current = next;
    }
    if visited != expected {
        return Err(CampaignError::DisconnectedChain {
            session: session.to_string(),
            visited,
            expected,
        });
    }
    Ok(visited)
}

/// Session promised by a receipt file name, rejecting anything that is not a
/// receipt.
fn session_of_receipt(file: &str) -> Option<String> {
    let stem = file.strip_suffix(".json")?;
    let session = match stem.split_once('.') {
        Some((session, generation)) => {
            if generation.is_empty() || !generation.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            session
        }
        None => stem,
    };
    if !is_session_id(session) {
        return None;
    }
    Some(session.to_string())
}

/// Whether `value` is shaped like an `OpenCode` session identifier, which is
/// both what a receipt file name promises and what a receipt body must claim.
#[must_use]
pub fn is_session_id(value: &str) -> bool {
    value
        .strip_prefix("ses_")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(is_id_byte))
}

/// Whether `value` names one source snapshot rather than a path that would
/// escape the campaign's `work/` directory.
#[must_use]
fn is_source_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || SOURCE_BYTES.contains(&byte))
}

/// Whether `value` is the `journal-*/<receipt>.json` address of another
/// receipt. Exactly one separator, and no `..` component, so a supersession
/// can only ever name a receipt inside the journals it already spans.
#[must_use]
fn is_journal_address(value: &str) -> bool {
    let mut parts = value.split('/');
    let (Some(journal), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    journal.starts_with("journal-")
        && !journal.contains("..")
        && !file.contains("..")
        && Path::new(file)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// A receipt field that is absent, which is distinct from one written with
/// the wrong shape: an operator has to be able to tell "not attested" from
/// "attested wrongly".
fn missing(key: &str, field: &str) -> CampaignError {
    CampaignError::MissingField {
        key: key.to_string(),
        field: field.to_string(),
    }
}

/// A receipt field written with the wrong type or shape.
fn invalid_field(key: &str, field: &str, problem: &str) -> CampaignError {
    CampaignError::InvalidField {
        key: key.to_string(),
        field: field.to_string(),
        problem: problem.to_string(),
    }
}

/// A required string field, checked for presence and type separately so the
/// two failures read differently.
fn text<'body>(body: &'body Value, field: &str, key: &str) -> Result<&'body str, CampaignError> {
    let value = body.get(field).ok_or_else(|| missing(key, field))?;
    value
        .as_str()
        .ok_or_else(|| invalid_field(key, field, "expected a string"))
}

/// Whether `value` is the 64 lowercase hex digits that
/// `crypto.createHash("sha256").digest("hex")` produces, and therefore
/// whether it can stand in as a digest anywhere in a campaign.
#[must_use]
pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A required digest field: see [`is_digest`].
fn digest<'body>(body: &'body Value, field: &str, key: &str) -> Result<&'body str, CampaignError> {
    let value = text(body, field, key)?;
    if !is_digest(value) {
        return Err(invalid_field(
            key,
            field,
            "expected 64 lowercase hex digits",
        ));
    }
    Ok(value)
}

/// A required non-negative integer field.
fn count(body: &Value, field: &str, key: &str) -> Result<i64, CampaignError> {
    let value = body.get(field).ok_or_else(|| missing(key, field))?;
    let integer = value
        .as_i64()
        .ok_or_else(|| invalid_field(key, field, "expected a non-negative integer"))?;
    if integer < 0 {
        return Err(invalid_field(key, field, "expected a non-negative integer"));
    }
    Ok(integer)
}

/// Bytes `OpenCode` allows inside a session identifier.
fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}
