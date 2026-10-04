//! Durable private source custody precedes model-facing context reduction.
//! Admission and continuation decisions are independent; rejected knowledge
//! never authorizes forgetting active work. All archive references are hashes.

pub mod cli;
mod planner;
mod replica;
mod worker;
pub use worker::Budget;

use std::{fs, io::Write, path::Path, time::Duration};
use chaosbox_core::sha256_hex;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Complete native records from one safe runtime boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    /// Adapter contract version.
    pub version: u32,
    /// Operator-pinned private visibility.
    pub scope: String,
    /// Explicit repository association.
    pub repo: String,
    /// Producer namespace.
    pub source: String,
    /// Native session identity.
    pub session: String,
    /// Ordered native messages, or one pre-bounding tool-result record.
    pub records: Vec<Value>,
}

/// Committed source manifest. Returned only after SQLite FULL commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Custody {
    /// Content-derived immutable capture identity.
    pub id: String,
    /// Number of complete records under custody.
    pub records: usize,
}

/// Deterministic checkpoint and auditable reduction dispositions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    /// Checkpoint text for `OpenCode`'s compaction result.
    pub summary: String,
    /// Complete retained tail, already represented by the checkpoint.
    pub recent: String,
    /// Committed capture receipt.
    pub custody: String,
    /// Content-derived plan identity.
    pub id: String,
    /// Source hashes containing externalized tool bodies or attachment bytes.
    pub externalized: Vec<String>,
    /// Source hashes omitted by independent continuation receipts.
    pub omitted: Vec<String>,
}

/// Private SQLite archive, work queue, receipts and knowledge outbox.
pub struct Journal {
    pub(super) db: Connection,
    pub(super) scope: String,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub(crate) fn private_database(root: &Path, name: &str) -> Result<Connection, String> {
    if !root.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(root).map_err(err)?;
        fs::File::open(root.parent().ok_or("archive root has no parent")?)
            .and_then(|f| f.sync_all())
            .map_err(err)?;
    }
    let directory = fs::symlink_metadata(root).map_err(err)?;
    if !directory.is_dir() {
        return Err("archive root must be a nonsymlink directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if directory.permissions().mode() & 0o077 != 0 {
            return Err("archive directory must be private (0700)".into());
        }
    }
    let path = root.join(name);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.flush().and_then(|()| file.sync_all()).map_err(err)?;
            fs::File::open(root)
                .and_then(|f| f.sync_all())
                .map_err(err)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(&path).map_err(err)?;
            if !meta.is_file() {
                return Err("archive database must be a nonsymlink regular file".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if meta.permissions().mode() & 0o077 != 0 || meta.nlink() != 1 {
                    return Err("archive database must be private and unlinked".into());
                }
            }
        }
        Err(e) => return Err(err(e)),
    }
    let db = Connection::open(path).map_err(err)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(err)?;
    db.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
    )
    .map_err(err)?;
    Ok(db)
}

impl Journal {
    /// Open a private archive; a root can never change visibility.
    pub fn open(root: &Path, scope: &str) -> Result<Self, String> {
        if !scope.starts_with("private:") || scope.len() <= 8 {
            return Err("private scope is required".into());
        }
        let mut db = private_database(root, "memory.sqlite")?;
        db.execute_batch(include_str!("schema.sql")).map_err(err)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let previous: Option<String> = tx
            .query_row("SELECT scope FROM identity", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        if previous.as_deref().is_some_and(|s| s != scope) {
            return Err("archive scope mismatch".into());
        }
        tx.execute("INSERT OR IGNORE INTO identity VALUES (1,?1)", [scope])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Self {
            db,
            scope: scope.into(),
        })
    }

    /// Commit immutable records and a pending job in one transaction.
    /// Variants retain separate hashes; identical captures are idempotent.
    pub fn capture(&mut self, input: &Capture) -> Result<Custody, String> {
        let raw = self.validate_capture(input)?;
        let id = sha256_hex(&["session-custody-v1", &raw]);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        for record in &input.records {
            let body = serde_json::to_string(record).map_err(err)?;
            let hash = sha256_hex(&[&body]);
            tx.execute(
                "INSERT OR IGNORE INTO blobs VALUES (?1,?2)",
                params![hash, body],
            )
            .map_err(err)?;
            if record["type"] == "tool-result" {
                let existing: Option<String> = tx.query_row("SELECT hash FROM tool_sources WHERE source=?1 AND session=?2 AND repo=?3 AND message=?4 AND call_id=?5",
                    params![input.source,input.session,input.repo,record["messageID"].as_str(),record["callID"].as_str()], |r| r.get(0)).optional().map_err(err)?;
                if existing.as_ref().is_some_and(|h| h != &hash) {
                    return Err("tool settlement changed after custody".into());
                }
                tx.execute(
                    "INSERT OR IGNORE INTO tool_sources VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        input.source,
                        input.session,
                        input.repo,
                        record["messageID"].as_str(),
                        record["callID"].as_str(),
                        hash
                    ],
                )
                .map_err(err)?;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO captures VALUES (?1,?2,?3,?4,?5,'pending')",
            params![id, input.source, input.session, input.repo, raw],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Custody {
            id,
            records: input.records.len(),
        })
    }

    fn validate_capture(&self, input: &Capture) -> Result<String, String> {
        if input.version != 1
            || input.scope != self.scope
            || input.records.is_empty()
            || input.records.len() > 100_000
            || [&input.repo, &input.source, &input.session]
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 500)
        {
            return Err("invalid capture identity or scope".into());
        }
        let raw = serde_json::to_string(input).map_err(err)?;
        if raw.len() > 64 * 1024 * 1024 {
            return Err("capture exceeds 64 MiB; partition at native message boundaries".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for record in &input.records {
            let id = record["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("native record lacks identity")?;
            if !ids.insert(id)
                || record["sessionID"]
                    .as_str()
                    .is_some_and(|s| s != input.session)
            {
                return Err("duplicate or cross-session record".into());
            }
            match record["type"].as_str() {
                Some("assistant")
                    if record
                        .pointer("/time/completed")
                        .and_then(Value::as_i64)
                        .is_none() =>
                {
                    return Err("unsettled assistant cannot be reduced".into());
                }
                Some("assistant") => {
                    for part in record["parts"]
                        .as_array()
                        .or_else(|| record["content"].as_array())
                        .into_iter()
                        .flatten()
                    {
                        if part["type"] == "tool"
                            && !matches!(
                                part.pointer("/state/status").and_then(Value::as_str),
                                Some("completed" | "error")
                            )
                        {
                            return Err("unsettled tool cannot be reduced".into());
                        }
                    }
                }
                Some("tool-result") => {
                    if !matches!(record["status"].as_str(), Some("completed" | "error"))
                        || ["messageID", "callID", "tool"]
                            .iter()
                            .any(|key| record[key].as_str().is_none_or(str::is_empty))
                    {
                        return Err("invalid settled tool result".into());
                    }
                }
                Some("shell")
                    if !matches!(
                        record["status"].as_str(),
                        Some("completed" | "error" | "exited" | "timeout" | "killed")
                    ) =>
                {
                    return Err("unsettled shell cannot be reduced".into());
                }
                Some(
                    "user" | "system" | "synthetic" | "compaction" | "shell" | "agent"
                    | "agent-switched" | "model-switched" | "model-selected" | "location-switched"
                    | "skill" | "idle",
                ) => (),
                _ => return Err("unsupported native record type".into()),
            }
        }
        Ok(raw)
    }

    /// Retrieve by opaque hash only, never a caller-selected file path.
    pub fn evidence(&self, hash: &str) -> Result<Value, String> {
        let blob: Option<String> = self
            .db
            .query_row("SELECT body FROM blobs WHERE hash=?1", [hash], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let Some(raw) = blob else {
            let text: String = self
                .db
                .query_row(
                    "SELECT body FROM normalized_sources WHERE hash=?1",
                    [hash],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if sha256_hex(&[&text]) != hash {
                return Err("normalized source digest mismatch".into());
            }
            return Ok(serde_json::json!({"source_jsonl":text,"sha256":hash}));
        };
        if sha256_hex(&[&raw]) != hash {
            return Err("archive content digest mismatch".into());
        }
        serde_json::from_str(&raw).map_err(err)
    }

    pub(super) fn load(&self, id: &str) -> Result<Capture, String> {
        let raw: String = self
            .db
            .query_row("SELECT body FROM captures WHERE id=?1", [id], |r| r.get(0))
            .map_err(err)?;
        if sha256_hex(&["session-custody-v1", &raw]) != id {
            return Err("custody manifest digest mismatch".into());
        }
        let capture: Capture = serde_json::from_str(&raw).map_err(err)?;
        if capture.scope != self.scope {
            return Err("custody scope mismatch".into());
        }
        for record in &capture.records {
            let hash = sha256_hex(&[&serde_json::to_string(record).map_err(err)?]);
            if self.evidence(&hash)? != *record {
                return Err("incomplete source custody".into());
            }
        }
        Ok(capture)
    }

    /// Unprocessed capture identities in durable order.
    pub fn pending(&self) -> Result<Vec<String>, String> {
        self.db
            .prepare("SELECT id FROM captures WHERE status!='assessed' ORDER BY rowid")
            .map_err(err)?
            .query_map([], |r| r.get(0))
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)
    }
}
