//! Private local scratch custody. Filesystem observations and session assertions
//! are independent evidence; neither age nor successful execution releases work.
pub mod assessment;
pub mod cli;

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use chaosbox_core::sha256_hex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Maximum wire payload; observation batches never include scratch file bodies.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Device, inode and birth time identify one allocation, independently of mtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Decimal device identifier (strings avoid JavaScript integer rounding).
    pub dev: String,
    /// Decimal inode identifier.
    pub ino: String,
    /// Nanoseconds since epoch; zero means birth time was unavailable.
    pub birth_ns: String,
}

/// A native text field, retained with its complete source record.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Native text projection; checkpoints, reasoning and synthetic text are refused.
    pub record: Value,
    /// JSON pointer to an original text field.
    pub pointer: String,
}

/// Append-only irreducible facts from the adapter or operator.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Event {
    /// Settled native text linked to a previously captured invocation.
    Context {
        /// Stable delivery identity.
        id: String,
        /// Existing invocation.
        invocation: String,
        /// Bounded original text projections.
        sources: Vec<Source>,
    },
    /// A caller-supplied artifact or completion reference; never verified by assertion.
    Link {
        /// Stable delivery identity.
        id: String,
        /// Allocation path.
        path: PathBuf,
        /// Exact allocation identity.
        identity: Identity,
        /// patch, test, commit, artifact or preservation.
        category: String,
        /// Source-authorized reference or destination.
        reference: String,
        /// Why this evidence applies to the workspace.
        description: String,
        /// Native session making the assertion, when captured by `OpenCode`.
        #[serde(default)]
        session: Option<String>,
        /// Native message making the assertion.
        #[serde(default)]
        message: Option<String>,
    },
    /// A tool or native shell has started.
    Begin {
        /// Stable delivery identity.
        id: String,
        /// Invocation identity, reused at settlement and by observations.
        invocation: String,
        /// Native session identity.
        session: String,
        /// Native assistant message, when available.
        message: String,
        /// Effective tool name.
        tool: String,
        /// Repository/location association.
        repo: String,
        /// Original command (or tool input serialized as JSON).
        command: String,
        /// Effective working directory.
        cwd: PathBuf,
        /// Exact bounded source fields explaining the work.
        #[serde(default)]
        sources: Vec<Source>,
        /// Native purpose fields omitted by the adapter's source budget.
        #[serde(default)]
        sources_omitted: usize,
    },
    /// Actual process/tool settlement; success is not a release.
    End {
        /// Stable delivery identity.
        id: String,
        /// Previously started invocation.
        invocation: String,
        /// exited, error, interrupted or unknown.
        outcome: String,
        /// Known process exit code.
        #[serde(default)]
        exit: Option<i64>,
        /// Explicit operator reconciliation assertion, never an observed exit.
        #[serde(default)]
        assertion: Option<String>,
    },
    /// Creation, disappearance or activity of one work allocation.
    Observe {
        /// Stable delivery identity.
        id: String,
        /// Absolute allocation path.
        path: PathBuf,
        /// Observed identity, retained for disappearance as well.
        identity: Identity,
        /// Whether the object exists at observation time.
        present: bool,
        /// True for observed mutations/use, false for reconciliation inventory.
        activity: bool,
        /// Possible originating invocations; temporal association is not proof.
        #[serde(default)]
        owners: Vec<String>,
    },
    /// An explicit disposition assertion, retained independently of observation.
    Annotation {
        /// Stable delivery identity.
        id: String,
        /// Existing allocation path.
        path: PathBuf,
        /// Exact allocation the annotation concerns.
        identity: Identity,
        /// open, needs-finalization, unknown or released.
        disposition: String,
        /// Purpose or remaining work, asserted by the caller.
        reason: String,
        /// Optional preservation receipt, e.g. a destination and content hash.
        #[serde(default)]
        receipt: Option<String>,
    },
    /// Observer heartbeat or a coverage gap; historical gaps remain visible.
    Coverage {
        /// Stable delivery identity.
        id: String,
        /// One adapter instance, independent of session/repository.
        observer: String,
        /// Whether it is currently observing the configured root.
        healthy: bool,
        /// Bounded diagnostic; never raw tool output.
        detail: String,
    },
}

/// Versioned write contract; all events in a batch commit together.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Wire schema, currently 1.
    pub version: u32,
    /// Operator-pinned visibility boundary.
    pub scope: String,
    /// Host namespace; local paths must never be matched across hosts.
    pub host: String,
    /// Explicit scratch root.
    pub root: PathBuf,
    /// Bounded batch of events.
    pub events: Vec<Event>,
}

/// Original allocation and an optional quarantine location used only for identity checks.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryPath {
    /// Original allocation path in the ledger.
    pub path: PathBuf,
    /// Current location after an identity-preserving quarantine rename.
    #[serde(default)]
    pub at: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    id: String,
    path: PathBuf,
    identity: Identity,
    present: bool,
    disposition: String,
    first_seen: i64,
    last_observed: i64,
    owners: BTreeSet<String>,
    annotations: Vec<Value>,
    #[serde(default)]
    links: Vec<Value>,
    #[serde(default)]
    assessment: Option<Value>,
    #[serde(default)]
    assessment_error: Option<Value>,
}

/// Durable operational ledger using the existing private SQLite custody helper.
pub struct Ledger {
    db: Connection,
    scope: String,
    work: PathBuf,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

fn absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("an absolute path without dot components is required".into());
    }
    Ok(())
}

fn within(path: &Path, root: &Path) -> Result<(), String> {
    absolute(root)?;
    absolute(path)?;
    if path == root || !path.starts_with(root) {
        return Err("allocation must be below scratch root".into());
    }
    Ok(())
}

/// Read metadata without following symlink components or crossing root devices.
pub fn identity(path: &Path, root: &Path) -> Result<Identity, String> {
    use std::os::unix::fs::MetadataExt;
    within(path, root)?;
    let mut cursor = PathBuf::from("/");
    let mut root_dev = None;
    for component in path.components().skip(1) {
        cursor.push(component);
        let meta = fs::symlink_metadata(&cursor).map_err(err)?;
        if meta.file_type().is_symlink() {
            return Err("symlink components are refused".into());
        }
        if cursor == root {
            root_dev = Some(meta.dev());
        }
        if root_dev.is_some_and(|dev| dev != meta.dev()) {
            return Err("scratch device boundary".into());
        }
        if cursor == path {
            if !meta.is_file() && !meta.is_dir() {
                return Err("special files are not allocations".into());
            }
            return Ok(Identity {
                dev: meta.dev().to_string(),
                ino: meta.ino().to_string(),
                birth_ns: meta
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or_else(|| "0".into(), |t| t.as_nanos().to_string()),
            });
        }
        if !meta.is_dir() {
            return Err("allocation ancestor is not a directory".into());
        }
    }
    Err("invalid allocation".into())
}

fn private_state(work: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    absolute(work)?;
    for path in [work.to_path_buf(), work.join("scratch.sqlite")] {
        let meta = fs::symlink_metadata(&path).map_err(err)?;
        if meta.file_type().is_symlink()
            || meta.permissions().mode() & 0o077 != 0
            || (path != work && (!meta.is_file() || meta.nlink() != 1))
            || (path == work && !meta.is_dir())
        {
            return Err("scratch ledger must be private and nonsymlink (0700/0600)".into());
        }
    }
    Ok(())
}

impl Ledger {
    /// Open/migrate a private writer. State belongs outside every tracked root.
    pub fn open(work: &Path, scope: &str) -> Result<Self, String> {
        absolute(work)?;
        if !scope.starts_with("private:") || scope.len() <= 8 {
            return Err("private scope required".into());
        }
        nonsymlink_ancestors(work)?;
        if !work.exists() {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(work)
                .map_err(err)?;
        }
        let mut db = crate::compaction::private_database(work, "scratch.sqlite")?;
        db.execute_batch(include_str!("schema.sql")).map_err(err)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let previous: Option<String> = tx
            .query_row("SELECT scope FROM identity", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        if previous.as_deref().is_some_and(|s| s != scope) {
            return Err("scratch scope mismatch".into());
        }
        tx.execute("INSERT OR IGNORE INTO identity VALUES (1,?1)", [scope])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Self {
            db,
            scope: scope.into(),
            work: work.into(),
        })
    }

    /// Open existing state read-only; querying never initializes or migrates it.
    pub fn read(work: &Path, scope: &str) -> Result<Self, String> {
        nonsymlink_ancestors(work)?;
        private_state(work)?;
        let db = Connection::open_with_flags(
            work.join("scratch.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(err)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(err)?;
        db.execute_batch("PRAGMA query_only=ON;").map_err(err)?;
        let stored: String = db
            .query_row("SELECT scope FROM identity", [], |r| r.get(0))
            .map_err(err)?;
        if scope != stored {
            return Err("scratch scope mismatch".into());
        }
        Ok(Self {
            db,
            scope: scope.into(),
            work: work.into(),
        })
    }

    /// Atomically append idempotent events and update their projections.
    pub fn record(&mut self, input: &Request) -> Result<Value, String> {
        if input.version != 1
            || input.scope != self.scope
            || input.host.is_empty()
            || input.host.len() > 200
            || input.events.is_empty()
            || input.events.len() > 256
            || serde_json::to_vec(input).map_err(err)?.len() > MAX_BYTES
        {
            return Err("invalid scratch batch or budget".into());
        }
        absolute(&input.root)?;
        if self.work.starts_with(&input.root) || input.root.starts_with(&self.work) {
            return Err("scratch state/root overlap".into());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        for event in &input.events {
            let body = serde_json::to_string(event).map_err(err)?;
            let value = serde_json::to_value(event).map_err(err)?;
            let event_id = value["id"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 500)
                .ok_or("invalid event id")?;
            let key = sha256_hex(&[
                "scratch-event-v1",
                &input.host,
                &input.root.to_string_lossy(),
                event_id,
            ]);
            let old: Option<String> = tx
                .query_row("SELECT body FROM events WHERE key=?1", [&key], |r| r.get(0))
                .optional()
                .map_err(err)?;
            if let Some(old) = old {
                if old != body {
                    return Err("scratch event identity reused with changed payload".into());
                }
                continue;
            }
            let at = now();
            Self::apply(&tx, input, event, at)?;
            tx.execute(
                "INSERT INTO events(key,host,root,body,recorded_at) VALUES (?1,?2,?3,?4,?5)",
                params![key, input.host, input.root.to_string_lossy(), body, at],
            )
            .map_err(err)?;
        }
        let generation: i64 = tx
            .query_row("SELECT coalesce(max(seq),0) FROM events", [], |r| r.get(0))
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(json!({"version":1,"generation":generation}))
    }

    fn apply(db: &Connection, input: &Request, event: &Event, at: i64) -> Result<(), String> {
        match event {
            Event::Context {
                invocation,
                sources,
                ..
            } => {
                if sources.len() > 8 {
                    return Err("context source budget exceeded".into());
                }
                let mut body =
                    load_invocation(db, &input.host, &input.root.to_string_lossy(), invocation)?
                        .ok_or("context lacks invocation")?;
                let mut retained: Vec<Source> =
                    serde_json::from_value(body["sources"].clone()).map_err(err)?;
                for source in sources {
                    source_text(source)?;
                    let raw = source.record.to_string();
                    db.execute(
                        "INSERT OR IGNORE INTO sources VALUES (?1,?2)",
                        params![sha256_hex(&["scratch-source-v1", &raw]), raw],
                    )
                    .map_err(err)?;
                    if !retained
                        .iter()
                        .any(|s| s.record == source.record && s.pointer == source.pointer)
                    {
                        retained.push(source.clone());
                    }
                }
                if retained.len() > 32 {
                    return Err("invocation context budget exceeded".into());
                }
                body["sources"] = json!(retained);
                db.execute(
                    "UPDATE invocations SET body=?4 WHERE host=?1 AND root=?2 AND id=?3",
                    params![
                        input.host,
                        input.root.to_string_lossy(),
                        invocation,
                        body.to_string()
                    ],
                )
                .map_err(err)?;
                Ok(())
            }
            Event::Link {
                path,
                identity: expected,
                category,
                reference,
                description,
                session,
                message,
                ..
            } => {
                within(path, &input.root)?;
                if !["patch", "test", "commit", "artifact", "preservation"]
                    .contains(&category.as_str())
                    || reference.trim().is_empty()
                    || reference.len() > 2000
                    || description.trim().is_empty()
                    || description.len() > 4000
                    || [session, message]
                        .iter()
                        .any(|s| s.as_ref().is_some_and(|v| v.is_empty() || v.len() > 500))
                {
                    return Err("invalid linked evidence".into());
                }
                let mut entry = load_entry(db, &input.host, &input.root.to_string_lossy(), path)?
                    .ok_or("allocation not recorded")?;
                if entry.identity != *expected || identity(path, &input.root)? != *expected {
                    return Err("allocation identity changed".into());
                }
                if entry.links.len() >= 64 {
                    return Err("linked evidence budget exceeded".into());
                }
                entry.links.push(json!({"category":category,"reference":reference,"description":description,"session":session,"message":message,"verified":false,"assertion":true}));
                if entry.disposition == "released" {
                    entry.disposition = "open".into();
                }
                save_entry(db, &input.host, &input.root.to_string_lossy(), &entry)
            }
            Event::Begin { .. } | Event::End { .. } => Self::apply_invocation(db, input, event, at),
            Event::Observe { .. } => Self::apply_observation(db, input, event, at),
            Event::Annotation { .. } => Self::apply_annotation(db, input, event, at),
            Event::Coverage {
                observer,
                healthy,
                detail,
                ..
            } => {
                if observer.is_empty() || observer.len() > 500 || detail.len() > 1000 {
                    return Err("invalid observer".into());
                }
                db.execute("INSERT INTO observers VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(host,root,id) DO UPDATE SET healthy=excluded.healthy,recorded_at=excluded.recorded_at,detail=excluded.detail,gaps=observers.gaps+excluded.gaps",
                    params![input.host,input.root.to_string_lossy(),observer,healthy,at,detail,u32::from(!healthy)]).map_err(err)?;
                Ok(())
            }
        }
    }

    fn apply_invocation(
        db: &Connection,
        input: &Request,
        event: &Event,
        at: i64,
    ) -> Result<(), String> {
        let host = &input.host;
        let root = input.root.to_string_lossy();
        match event {
            Event::Begin {
                invocation,
                session,
                message,
                tool,
                repo,
                command,
                cwd,
                sources,
                sources_omitted,
                ..
            } => {
                if [invocation, session, tool, repo]
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 500)
                    || command.len() > 32_000
                    || message.len() > 500
                    || sources.len() > 8
                {
                    return Err("invalid invocation".into());
                }
                absolute(cwd)?;
                for source in sources {
                    source_text(source)?;
                    let raw = source.record.to_string();
                    let hash = sha256_hex(&["scratch-source-v1", &raw]);
                    db.execute(
                        "INSERT OR IGNORE INTO sources VALUES (?1,?2)",
                        params![hash, raw],
                    )
                    .map_err(err)?;
                }
                let body = json!({"id":invocation,"session":session,"message":message,"tool":tool,"repo":repo,
                    "command":command,"cwd":cwd,"sources":sources,"sources_omitted":sources_omitted,"status":"running","started_at":at});
                db.execute(
                    "INSERT INTO invocations VALUES (?1,?2,?3,?4)",
                    params![host, root, invocation, body.to_string()],
                )
                .map_err(err)?;
            }
            Event::End {
                invocation,
                outcome,
                exit,
                assertion,
                ..
            } => {
                if !["exited", "error", "interrupted", "unknown"].contains(&outcome.as_str()) {
                    return Err("invalid settlement".into());
                }
                let mut body = load_invocation(db, host, &root, invocation)?
                    .ok_or("settlement lacks begin")?;
                if assertion
                    .as_ref()
                    .is_some_and(|s| s.trim().is_empty() || s.len() > 4000)
                {
                    return Err("invalid reconciliation assertion".into());
                }
                if body["status"] != "running"
                    && !(body["status"] == "unknown" && assertion.is_some())
                {
                    return Err("invocation already settled".into());
                }
                body["status"] = json!(outcome);
                body["exit"] = json!(exit);
                body["completed_at"] = json!(at);
                body["reconciliation_assertion"] = json!(assertion);
                db.execute(
                    "UPDATE invocations SET body=?4 WHERE host=?1 AND root=?2 AND id=?3",
                    params![host, root, invocation, body.to_string()],
                )
                .map_err(err)?;
            }
            _ => return Err("expected invocation event".into()),
        }
        Ok(())
    }

    fn apply_observation(
        db: &Connection,
        input: &Request,
        event: &Event,
        at: i64,
    ) -> Result<(), String> {
        let host = &input.host;
        let root = input.root.to_string_lossy();
        match event {
            Event::Observe {
                path,
                identity,
                present,
                activity,
                owners,
                ..
            } => {
                within(path, &input.root)?;
                validate_identity(identity)?;
                if owners.len() > 64 {
                    return Err("too many possible owners".into());
                }
                for owner in owners {
                    if load_invocation(db, host, &root, owner)?.is_none() {
                        return Err("unknown invocation owner".into());
                    }
                }
                let mut entry = load_entry(db, host, &root, path)?
                    .filter(|e| e.identity == *identity)
                    .unwrap_or_else(|| Entry {
                        id: sha256_hex(&[
                            "scratch-allocation-v1",
                            host,
                            &root,
                            &path.to_string_lossy(),
                            &identity.dev,
                            &identity.ino,
                            &identity.birth_ns,
                        ]),
                        path: path.clone(),
                        identity: identity.clone(),
                        present: *present,
                        disposition: if owners.is_empty() {
                            "unknown".into()
                        } else {
                            "open".into()
                        },
                        first_seen: at,
                        last_observed: at,
                        owners: BTreeSet::new(),
                        annotations: Vec::new(),
                        links: Vec::new(),
                        assessment: None,
                        assessment_error: None,
                    });
                entry.present = *present;
                entry.last_observed = at;
                entry.owners.extend(owners.iter().cloned());
                if entry.owners.len() > 64 {
                    return Err("allocation owner budget exceeded".into());
                }
                if *activity && entry.disposition == "released" {
                    entry.disposition = "open".into();
                }
                save_entry(db, host, &root, &entry)?;
            }
            _ => return Err("expected observation event".into()),
        }
        Ok(())
    }

    fn apply_annotation(
        db: &Connection,
        input: &Request,
        event: &Event,
        at: i64,
    ) -> Result<(), String> {
        let host = &input.host;
        let root = input.root.to_string_lossy();
        match event {
            Event::Annotation {
                path,
                identity: expected,
                disposition,
                reason,
                receipt,
                ..
            } => {
                within(path, &input.root)?;
                if !["unknown", "open", "needs-finalization", "released"]
                    .contains(&disposition.as_str())
                    || reason.trim().is_empty()
                    || reason.len() > 4000
                    || receipt.as_ref().is_some_and(|r| r.len() > 4000)
                {
                    return Err("invalid annotation".into());
                }
                let mut entry =
                    load_entry(db, host, &root, path)?.ok_or("allocation not recorded")?;
                if entry.identity != *expected || identity(path, &input.root)? != *expected {
                    return Err("allocation identity changed".into());
                }
                if disposition == "released" {
                    for owner in &entry.owners {
                        if load_invocation(db, host, &root, owner)?
                            .is_none_or(|v| v["status"] == "running" || v["status"] == "unknown")
                        {
                            return Err("active or unresolved invocation prevents release".into());
                        }
                    }
                }
                entry.disposition.clone_from(disposition);
                if entry.annotations.len() == 128 {
                    entry.annotations.remove(0);
                }
                entry.annotations.push(json!({"disposition":disposition,"reason":reason,"receipt":receipt,"receipt_verified":false,"assertion":true,"at":at}));
                save_entry(db, host, &root, &entry)?;
            }
            _ => return Err("expected annotation event".into()),
        }
        Ok(())
    }

    /// Add an explicit operator annotation bound to the current filesystem object.
    pub fn annotate(
        &mut self,
        host: &str,
        root: &Path,
        path: &Path,
        disposition: &str,
        reason: &str,
        receipt: Option<&str>,
    ) -> Result<Value, String> {
        let event = Event::Annotation {
            id: format!(
                "operator:{}:{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(err)?
                    .as_nanos()
            ),
            path: path.into(),
            identity: identity(path, root)?,
            disposition: disposition.into(),
            reason: reason.into(),
            receipt: receipt.map(str::to_owned),
        };
        self.record(&Request {
            version: 1,
            scope: self.scope.clone(),
            host: host.into(),
            root: root.into(),
            events: vec![event],
        })
    }

    /// Explicitly reconcile a process which no longer has a live exit observer.
    pub fn resolve(
        &mut self,
        host: &str,
        root: &Path,
        invocation: &str,
        reason: &str,
    ) -> Result<Value, String> {
        self.record(&Request {
            version: 1,
            scope: self.scope.clone(),
            host: host.into(),
            root: root.into(),
            events: vec![Event::End {
                id: format!(
                    "resolve:{}:{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(err)?
                        .as_nanos()
                ),
                invocation: invocation.into(),
                outcome: "interrupted".into(),
                exit: None,
                assertion: Some(reason.into()),
            }],
        })
    }

    /// One consistent bounded snapshot for Doty, including ancestor/descendant holds.
    pub fn query(
        &self,
        host: &str,
        root: &Path,
        paths: &[PathBuf],
        limit: usize,
    ) -> Result<Value, String> {
        self.query_at(
            host,
            root,
            &paths
                .iter()
                .map(|path| QueryPath {
                    path: path.clone(),
                    at: None,
                })
                .collect::<Vec<_>>(),
            limit,
        )
    }

    /// The same snapshot at identity-preserving quarantine locations.
    pub fn query_at(
        &self,
        host: &str,
        root: &Path,
        paths: &[QueryPath],
        limit: usize,
    ) -> Result<Value, String> {
        absolute(root)?;
        if host.is_empty()
            || host.len() > 200
            || paths.len() > 256
            || !(1..=10_000).contains(&limit)
        {
            return Err("invalid query budget".into());
        }
        for path in paths {
            within(&path.path, root)?;
            if let Some(at) = &path.at {
                within(at, root)?;
            }
        }
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let root_text = root.to_string_lossy();
        let generation: i64 = tx
            .query_row("SELECT coalesce(max(seq),0) FROM events", [], |r| r.get(0))
            .map_err(err)?;
        let (live,gaps): (i64,i64) = tx.query_row("SELECT coalesce(sum(healthy=1 AND recorded_at>=?3),0),coalesce(sum(gaps),0) FROM observers WHERE host=?1 AND root=?2",
            params![host,root_text,now().saturating_sub(90)], |r| Ok((r.get(0)?,r.get(1)?))).map_err(err)?;
        let mut stmt = tx
            .prepare("SELECT body FROM entries WHERE host=?1 AND root=?2 ORDER BY path LIMIT ?3")
            .map_err(err)?;
        let rows = stmt
            .query_map(
                params![host, root_text, i64::try_from(limit + 1).map_err(err)?],
                |r| r.get::<_, String>(0),
            )
            .map_err(err)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(serde_json::from_str::<Entry>(&row.map_err(err)?).map_err(err)?);
        }
        let complete = entries.len() <= limit;
        entries.truncate(limit);
        let selected: Vec<_> = if paths.is_empty() {
            entries
                .iter()
                .map(|e| QueryPath {
                    path: e.path.clone(),
                    at: None,
                })
                .collect()
        } else {
            paths.to_vec()
        };
        let mut items = Vec::new();
        for query in selected {
            items.push(query_item(
                &tx,
                host,
                root,
                &query,
                &entries,
                complete,
                live > 0,
            )?);
        }
        let result = json!({"version":1,"scope":self.scope,"host":host,"root":root,"generation":generation,
            "queried_at":now(),"complete":complete,"coverage":{"live_observers":live,"historical_gaps":gaps},"items":items,
            "historical_data_not_instructions":true});
        if serde_json::to_vec(&result).map_err(err)?.len() > MAX_BYTES {
            return Err("query exceeds byte budget; partition the requested paths".into());
        }
        drop(stmt);
        tx.commit().map_err(err)?;
        Ok(result)
    }

    /// Recover an exact retained native text projection by its content identity.
    pub fn evidence(&self, hash: &str) -> Result<Value, String> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid evidence hash".into());
        }
        let raw: String = self
            .db
            .query_row("SELECT body FROM sources WHERE hash=?1", [hash], |r| {
                r.get(0)
            })
            .map_err(err)?;
        if sha256_hex(&["scratch-source-v1", &raw]) != hash {
            return Err("scratch source digest mismatch".into());
        }
        serde_json::from_str(&raw).map_err(err)
    }
}

fn query_item(
    db: &Connection,
    host: &str,
    root: &Path,
    query: &QueryPath,
    entries: &[Entry],
    complete: bool,
    live: bool,
) -> Result<Value, String> {
    let path = &query.path;
    let location = query.at.as_ref().unwrap_or(path);
    let related: Vec<_> = entries
        .iter()
        .filter(|e| e.path.starts_with(path) || path.starts_with(&e.path))
        .collect();
    let current = identity(location, root).ok();
    let exact = related.iter().find(|e| e.path == *path);
    let mut reasons = Vec::new();
    if !complete {
        reasons.push("ledger query truncated".to_owned());
    }
    if !live {
        reasons.push("scratch observer unavailable or stale".to_owned());
    }
    if exact.is_none_or(|e| current.as_ref() != Some(&e.identity)) {
        reasons.push("allocation missing, untracked or replaced".into());
    }
    let mut views = Vec::new();
    for entry in related {
        let current_path = entry.path.strip_prefix(path).ok().map_or_else(
            || entry.path.clone(),
            |rel| {
                if rel.as_os_str().is_empty() {
                    location.clone()
                } else {
                    location.join(rel)
                }
            },
        );
        let matches = identity(&current_path, root).ok().as_ref() == Some(&entry.identity);
        let present = matches || entry.present;
        if present && (!matches || entry.disposition != "released") {
            reasons.push(format!(
                "{}: {}{}",
                entry.path.display(),
                entry.disposition,
                if matches { "" } else { " (identity changed)" }
            ));
        }
        let mut invocations = Vec::new();
        for owner in &entry.owners {
            if let Some(mut invocation) = load_invocation(db, host, &root.to_string_lossy(), owner)?
            {
                if present
                    && (invocation["status"] == "running" || invocation["status"] == "unknown")
                {
                    reasons.push(format!("invocation {owner} is active or unresolved"));
                }
                let sources: Vec<Source> =
                    serde_json::from_value(invocation["sources"].clone()).map_err(err)?;
                invocation["sources"] = json!(sources.iter().map(source_packet).collect::<Result<
                    Vec<_>,
                    _,
                >>(
                )?);
                invocations.push(invocation);
            }
        }
        let mut value = serde_json::to_value(entry).map_err(err)?;
        value["identity_matches"] = json!(matches);
        value["attribution"] = json!(if entry.owners.is_empty() {
            "unknown"
        } else {
            "temporal-association"
        });
        let digest = assessment::evidence_digest(host, root, entry, &invocations);
        value["invocations"] = json!(invocations);
        if let Some(receipt) = &entry.assessment {
            value["assessment"] = receipt.clone();
            value["assessment"]["fresh"] = json!(receipt["evidence_digest"] == digest);
        }
        value["assessment_state"] = json!(if entry
            .assessment
            .as_ref()
            .is_some_and(|r| r["evidence_digest"] == digest)
        {
            "current"
        } else if entry
            .assessment_error
            .as_ref()
            .is_some_and(|r| r["evidence_digest"] == digest)
        {
            "failed-or-interrupted"
        } else {
            "pending"
        });
        views.push(value);
    }
    Ok(
        json!({"path":path,"at":query.at,"blocked":!reasons.is_empty(),"reasons":reasons,"entries":views}),
    )
}

fn validate_identity(i: &Identity) -> Result<(), String> {
    if [&i.dev, &i.ino, &i.birth_ns]
        .iter()
        .any(|s| s.is_empty() || s.len() > 40 || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid filesystem identity".into());
    }
    Ok(())
}

fn source_text(source: &Source) -> Result<&str, String> {
    if !matches!(source.record["type"].as_str(), Some("user" | "assistant"))
        || source.record["id"].as_str().is_none_or(str::is_empty)
        || source
            .record
            .pointer("/metadata/chaosboxDerived")
            .and_then(Value::as_bool)
            == Some(true)
        || source.pointer.len() > 500
        || source.record.to_string().len() > 128 * 1024
    {
        return Err("invalid native purpose source".into());
    }
    let valid = (source.record["type"] == "user"
        && (source.pointer == "/text" || {
            let pieces: Vec<_> = source.pointer.split('/').collect();
            pieces.len() == 5
                && pieces[1] == "prompt"
                && pieces[2] == "parts"
                && pieces[4] == "text"
                && pieces[3]
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| {
                        source
                            .record
                            .pointer("/prompt/parts")
                            .and_then(|p| p.get(i))
                    })
                    .is_some_and(|v| v["type"] == "text")
        }))
        || ["content", "parts"].iter().any(|key| {
            let pieces: Vec<_> = source.pointer.split('/').collect();
            pieces.len() == 4
                && pieces[1] == *key
                && pieces[3] == "text"
                && pieces[2]
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| source.record[*key].get(i))
                    .is_some_and(|v| v["type"] == "text")
        });
    if !valid {
        return Err("purpose pointer is not an original text field".into());
    }
    source
        .record
        .pointer(&source.pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "purpose text missing".into())
}

fn nonsymlink_ancestors(path: &Path) -> Result<(), String> {
    absolute(path)?;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("scratch custody symlink ancestor refused".into());
            }
            Ok(meta) if !meta.is_dir() => {
                return Err("scratch custody ancestor is not a directory".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(e)),
        }
    }
    Ok(())
}

fn source_packet(source: &Source) -> Result<Value, String> {
    let text = source_text(source)?;
    let quote: String = text.chars().take(2000).collect();
    Ok(
        json!({"hash":sha256_hex(&["scratch-source-v1",&source.record.to_string()]),"message":source.record["id"],
        "speaker":source.record["type"],"pointer":source.pointer,"quote":quote,"truncated":text.chars().count()>2000}),
    )
}

fn load_invocation(
    db: &Connection,
    host: &str,
    root: &str,
    id: &str,
) -> Result<Option<Value>, String> {
    let raw: Option<String> = db
        .query_row(
            "SELECT body FROM invocations WHERE host=?1 AND root=?2 AND id=?3",
            params![host, root, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    raw.map(|s| serde_json::from_str(&s).map_err(err))
        .transpose()
}

fn load_entry(
    db: &Connection,
    host: &str,
    root: &str,
    path: &Path,
) -> Result<Option<Entry>, String> {
    let raw: Option<String> = db
        .query_row(
            "SELECT body FROM entries WHERE host=?1 AND root=?2 AND path=?3",
            params![host, root, path.to_string_lossy()],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    raw.map(|s| serde_json::from_str(&s).map_err(err))
        .transpose()
}

fn save_entry(db: &Connection, host: &str, root: &str, entry: &Entry) -> Result<(), String> {
    db.execute("INSERT INTO entries VALUES (?1,?2,?3,?4) ON CONFLICT(host,root,path) DO UPDATE SET body=excluded.body",
        params![host,root,entry.path.to_string_lossy(),serde_json::to_string(entry).map_err(err)?]).map_err(err)?;
    Ok(())
}
