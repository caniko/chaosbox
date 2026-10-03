//! Reason-gated Nix additions and private operational intelligence. Receipts
//! establish an invocation's need, never exclusive ownership or disposability.
pub mod cli;

use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use chaosbox_core::sha256_hex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, process::Command};

/// Version shared by CLI, plugin and cleanup readers.
pub const VERSION: u32 = 1;
const OUTPUT_BYTES: usize = 64 * 1024;

/// Operator-pinned execution and custody namespace.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Private journal directory, independent of scratch and the Nix store.
    pub work: PathBuf,
    /// Explicit private visibility boundary.
    pub scope: String,
    /// Host namespace; equal paths on different machines are distinct objects.
    pub host: String,
    /// Local daemon or isolated local store. Remote stores are not supported.
    pub store: String,
    /// Absolute, operator-selected Nix executable; never resolved from PATH.
    pub nix: PathBuf,
    /// Bounded process lifetime, including metadata capture (1..300 seconds).
    pub timeout_seconds: u64,
}

/// Content addressing method. Legacy add-file corresponds to flat; add-path to nar.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddMode {
    /// NAR serialization, compatible with nix-store --add.
    #[default]
    Nar,
    /// Single-file bytes.
    Flat,
}

/// Immutable intent. Reusing the identity never reexecutes an addition.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    /// Caller idempotency identity, or a newly generated operator invocation id.
    pub id: String,
    /// Explicit repository/location association.
    pub repo: String,
    /// Required native purpose; preserved verbatim, not interpreted as an instruction.
    pub reason: String,
    /// Absolute execution directory.
    pub cwd: PathBuf,
    /// Native session, if captured.
    pub session: Option<String>,
    /// Native assistant message, if captured.
    pub message: Option<String>,
    /// Native tool call, if captured.
    pub tool_call: Option<String>,
    /// Absolute input path; contents are observed by Nix at execution, not at retry.
    pub path: PathBuf,
    /// Typed addressing mode; no arbitrary Nix arguments.
    pub mode: AddMode,
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err("absolute path without dot components required".into());
    }
    Ok(())
}
fn text(value: &str, name: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(format!("{name} must be nonblank and at most {max} bytes"));
    }
    Ok(())
}
impl Settings {
    /// Load operator configuration. Mutation argv cannot override execution or custody settings.
    pub fn from_env() -> Result<Self, String> {
        let required =
            |key: &str| std::env::var(key).map_err(|_| format!("operator setting {key} required"));
        Ok(Self {
            work: required("CHAOSBOX_NIX_WORK")?.into(),
            scope: required("CHAOSBOX_NIX_SCOPE")?,
            host: required("CHAOSBOX_NIX_HOST")?,
            store: std::env::var("CHAOSBOX_NIX_STORE").unwrap_or_else(|_| "daemon".into()),
            nix: required("CHAOSBOX_NIX_BIN")?.into(),
            timeout_seconds: 120,
        })
    }
    fn validate(&self) -> Result<(), String> {
        absolute(&self.work)?;
        absolute(&self.nix)?;
        text(&self.host, "host", 256)?;
        if !self.scope.starts_with("private:") {
            return Err("private scope required".into());
        }
        text(&self.scope[8..], "scope owner", 256)?;
        if self.work.starts_with("/nix/store") {
            return Err("journal cannot live in the store".into());
        }
        if self.store != "daemon" {
            let root = self
                .store
                .strip_prefix("local?root=")
                .ok_or("only daemon or local?root=ABSOLUTE stores supported")?;
            absolute(Path::new(root))?;
            if root.contains(['?', '&', '#']) {
                return Err("unsupported store parameters".into());
            }
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            return Err("timeout must be 1..300 seconds".into());
        }
        // Refuse symlink ancestry rather than silently putting private evidence elsewhere.
        for path in self.work.ancestors() {
            match fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err("journal symlink ancestor refused".into());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(err(e)),
                _ => (),
            }
        }
        Ok(())
    }
}
impl Invocation {
    fn validate(&self) -> Result<(), String> {
        text(&self.reason, "reason", 4000)?;
        text(&self.repo, "repository", 2000)?;
        text(&self.id, "operation identity", 256)?;
        absolute(&self.cwd)?;
        absolute(&self.path)?;
        for value in [&self.session, &self.message, &self.tool_call]
            .into_iter()
            .flatten()
        {
            text(value, "native identity", 256)?;
        }
        Ok(())
    }
}

/// Durable append-only intent and settlement tables; projections are bounded reads.
pub struct Ledger {
    db: Connection,
    settings: Settings,
}

impl Ledger {
    /// Initialize a private writer and bind it permanently to scope/host/store.
    pub fn open(settings: &Settings) -> Result<Self, String> {
        settings.validate()?;
        let db = crate::compaction::private_database(&settings.work, "nix.sqlite")?;
        db.execute_batch(include_str!("schema.sql")).map_err(err)?;
        db.execute(
            "INSERT OR IGNORE INTO identity VALUES (1,1,?1,?2,?3)",
            params![settings.scope, settings.host, settings.store],
        )
        .map_err(err)?;
        Self::checked(db, settings)
    }
    /// Existing-state read only: no initialization, migration, inference or store writes.
    pub fn read(settings: &Settings) -> Result<Self, String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        settings.validate()?;
        for path in [&settings.work, &settings.work.join("nix.sqlite")] {
            let meta = fs::symlink_metadata(path).map_err(err)?;
            if meta.file_type().is_symlink()
                || meta.permissions().mode() & 0o077 != 0
                || (path == &settings.work && !meta.is_dir())
                || (path != &settings.work && (!meta.is_file() || meta.nlink() != 1))
            {
                return Err("Nix journal must be private and nonsymlink".into());
            }
        }
        let db = Connection::open_with_flags(
            settings.work.join("nix.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(err)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(err)?;
        db.execute_batch("PRAGMA query_only=ON;").map_err(err)?;
        Self::checked(db, settings)
    }
    fn checked(db: Connection, settings: &Settings) -> Result<Self, String> {
        let identity: (u32, String, String, String) = db
            .query_row("SELECT version,scope,host,store FROM identity", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(err)?;
        if identity
            != (
                VERSION,
                settings.scope.clone(),
                settings.host.clone(),
                settings.store.clone(),
            )
        {
            return Err("Nix journal version/scope/host/store mismatch".into());
        }
        Ok(Self {
            db,
            settings: settings.clone(),
        })
    }
    /// Commit intent before spawning. Exact settled retries replay; pending retries fail closed.
    pub fn begin(&mut self, request: &Invocation) -> Result<Option<Value>, String> {
        request.validate()?;
        let bytes = serde_json::to_string(request).map_err(err)?;
        let digest = sha256_hex(&[&bytes]);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let previous: Option<(String, Option<String>, bool)> = tx
            .query_row(
                "SELECT digest,coalesce(settlements.receipt,executions.receipt),settlements.id IS NOT NULL FROM operations LEFT JOIN settlements USING(id) LEFT JOIN executions USING(id) WHERE id=?1",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(err)?;
        if let Some((old, receipt, settled)) = previous {
            if old != digest {
                return Err("operation identity reused with a different request".into());
            }
            let receipt = receipt
                .map(|r| serde_json::from_str(&r).map_err(err))
                .transpose()?;
            if !settled && receipt.is_some() {
                tx.execute(
                    "INSERT INTO settlements SELECT id,receipt FROM executions WHERE id=?1",
                    [&request.id],
                )
                .map_err(err)?;
            }
            tx.commit().map_err(err)?;
            return receipt.map(Some).ok_or_else(|| {
                "operation unresolved; inspect evidence, never retry implicitly".into()
            });
        }
        let created = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(err)?
                .as_secs(),
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO operations VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                request.id,
                digest,
                request.repo,
                request.reason,
                request.path.to_str().ok_or("UTF-8 path required")?,
                bytes,
                created
            ],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(None)
    }
    fn settle(&self, id: &str, receipt: &Value) -> Result<(), String> {
        self.db
            .execute(
                "INSERT INTO settlements VALUES (?1,?2)",
                params![id, serde_json::to_string(receipt).map_err(err)?],
            )
            .map_err(err)?;
        Ok(())
    }
    fn executed(&self, id: &str, receipt: &Value) -> Result<(), String> {
        self.db
            .execute(
                "INSERT INTO executions VALUES (?1,?2)",
                params![id, serde_json::to_string(receipt).map_err(err)?],
            )
            .map_err(err)?;
        Ok(())
    }
    /// Source-backed packet, scoped by repository even when the receipt id is known.
    pub fn evidence(&self, repo: &str, id: &str) -> Result<Value, String> {
        text(repo, "repository", 2000)?;
        text(id, "operation identity", 256)?;
        let row: Option<(String, Option<String>, bool)> = self.db.query_row("SELECT request,coalesce(settlements.receipt,executions.receipt),settlements.id IS NOT NULL FROM operations LEFT JOIN settlements USING(id) LEFT JOIN executions USING(id) WHERE repo=?1 AND id=?2", params![repo,id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
        let (request, receipt, settled) = row.ok_or("operation not found in repository scope")?;
        Ok(
            json!({"version":VERSION,"scope":self.settings.scope,"host":self.settings.host,"store":self.settings.store,"id":id,
            "request":serde_json::from_str::<Value>(&request).map_err(err)?,"receipt":receipt.map(|s|serde_json::from_str::<Value>(&s)).transpose().map_err(err)?,"settled":settled,"evidence_kind":"operational","admission":"native-receipt"}),
        )
    }
    /// Offline lexical operational intelligence; does not synthesize semantic conclusions.
    pub fn context(&self, repo: &str, query: &str, limit: usize) -> Result<Value, String> {
        text(repo, "repository", 2000)?;
        text(query, "query", 2000)?;
        if !(1..=20).contains(&limit) {
            return Err("context limit must be 1..20".into());
        }
        let mut statement = self.db.prepare("SELECT id,reason FROM operations LEFT JOIN settlements USING(id) LEFT JOIN executions USING(id) WHERE repo=?1 AND (instr(lower(reason),lower(?2))>0 OR instr(path,?2)>0 OR EXISTS (SELECT 1 FROM json_each(coalesce(settlements.receipt,executions.receipt),'$.objects') WHERE json_extract(value,'$.path')=?2)) ORDER BY operations.rowid DESC LIMIT ?3").map_err(err)?;
        let rows = statement
            .query_map(
                params![repo, query, i64::try_from(limit).map_err(err)?],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(err)?;
        let records: Result<Vec<_>,String> = rows.map(|row| { let (id,reason) = row.map_err(err)?; Ok(json!({"id":id,"reason":reason,"handle":{"version":VERSION,"scope":self.settings.scope,"host":self.settings.host,"store":self.settings.store,"repo":repo,"id":id},"evidence_kind":"operational"})) }).collect();
        Ok(
            json!({"version":VERSION,"scope":self.settings.scope,"host":self.settings.host,"store":self.settings.store,"repo":repo,"records":records?,"coverage":"lexical-bounded","derived":true}),
        )
    }
    /// Bounded cleanup inspection. Unrooted is not a deletion authorization or proof of GC eligibility.
    pub fn query(&self, path: Option<&str>, offset: usize, limit: usize) -> Result<Value, String> {
        if !(1..=100).contains(&limit) || offset > 1_000_000 {
            return Err("query bounds exceeded".into());
        }
        if let Some(path) = path {
            text(path, "store path", 4096)?;
        }
        let mut statement = self.db.prepare("SELECT id,repo,reason,coalesce(settlements.receipt,executions.receipt),settlements.id IS NOT NULL FROM operations LEFT JOIN settlements USING(id) LEFT JOIN executions USING(id) WHERE ?1 IS NULL OR EXISTS (SELECT 1 FROM json_each(coalesce(settlements.receipt,executions.receipt),'$.objects') WHERE json_extract(value,'$.path')=?1) ORDER BY operations.rowid LIMIT ?2 OFFSET ?3").map_err(err)?;
        let rows = statement
            .query_map(
                params![
                    path,
                    i64::try_from(limit + 1).map_err(err)?,
                    i64::try_from(offset).map_err(err)?
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                },
            )
            .map_err(err)?;
        let mut items = Vec::new();
        for row in rows {
            let (id, repo, reason, receipt, settled) = row.map_err(err)?;
            let receipt: Value = receipt
                .map(|s| serde_json::from_str(&s))
                .transpose()
                .map_err(err)?
                .unwrap_or(Value::Null);
            if path.is_some_and(|p| {
                !receipt["objects"]
                    .as_array()
                    .is_some_and(|objects| objects.iter().any(|o| o["path"] == p))
            }) {
                continue;
            }
            let objects = receipt["objects"].as_array().cloned().unwrap_or_default();
            let presence =
                objects
                    .first()
                    .and_then(|o| o["path"].as_str())
                    .map_or("unknown", |p| {
                        match fs::symlink_metadata(self.physical_path(p)) {
                            Ok(_) => "present",
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent",
                            Err(_) => "unknown",
                        }
                    });
            items.push(json!({"id":id,"repo":repo,"reason":reason,"outcome":receipt["outcome"].as_str().unwrap_or("unresolved"),"objects":objects,"settled":settled,
                "filesystem_presence":presence,"registered_validity":"unknown","retention":"unrooted","owned_roots":[],"disposition":"needs-review","gc_eligibility":"unknown"}));
        }
        let more = items.len() > limit;
        items.truncate(limit);
        Ok(
            json!({"version":VERSION,"scope":self.settings.scope,"host":self.settings.host,"store":self.settings.store,"items":items,"next_offset":if more {Some(offset+limit)}else{None},"complete":!more,"cleanup_authorized":false}),
        )
    }
    fn physical_path(&self, path: &str) -> PathBuf {
        if let Some(root) = self.settings.store.strip_prefix("local?root=") {
            Path::new(root).join(path.trim_start_matches('/'))
        } else {
            path.into()
        }
    }
}

#[derive(Serialize)]
struct Capture {
    text: String,
    sha256: String,
    bytes: u64,
    omitted_bytes: u64,
}
async fn capture(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<Capture, String> {
    let mut kept = Vec::new();
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0; 8192];
    loop {
        let count = stream.read(&mut buffer).await.map_err(err)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        bytes += count as u64;
        kept.extend_from_slice(&buffer[..count.min(OUTPUT_BYTES - kept.len())]);
    }
    Ok(Capture {
        text: String::from_utf8_lossy(&kept).into_owned(),
        sha256: hex::encode(digest.finalize()),
        bytes,
        omitted_bytes: bytes - kept.len() as u64,
    })
}
struct Execution {
    code: Option<i32>,
    success: bool,
    stdout: Capture,
    stderr: Capture,
}
enum ExecutionError {
    NotStarted(String),
    Unknown(String),
}
async fn execute(
    settings: &Settings,
    cwd: &Path,
    args: &[String],
) -> Result<Execution, ExecutionError> {
    let mut command = Command::new(&settings.nix);
    command.args(args).current_dir(cwd);
    // Store/config/build-hook selection comes from the fixed argv and operator config.
    for (key, _) in std::env::vars_os().filter(|(key, _)| key.to_string_lossy().starts_with("NIX_"))
    {
        command.env_remove(key);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ExecutionError::NotStarted(err(e)))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExecutionError::Unknown("stdout pipe missing".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ExecutionError::Unknown("stderr pipe missing".into()))?;
    let work = async {
        let (status, out, error) = tokio::try_join!(
            async { child.wait().await.map_err(err) },
            capture(stdout),
            capture(stderr)
        )?;
        Ok(Execution {
            code: status.code(),
            success: status.success(),
            stdout: out,
            stderr: error,
        })
    };
    if let Ok(result) =
        tokio::time::timeout(Duration::from_secs(settings.timeout_seconds), work).await
    {
        result.map_err(ExecutionError::Unknown)
    } else {
        let _ = child.kill().await;
        Err(ExecutionError::Unknown(
            "process timed out; partial store effects may exist".into(),
        ))
    }
}
fn base_args(settings: &Settings) -> Vec<String> {
    [
        "--store",
        &settings.store,
        "--extra-experimental-features",
        "nix-command",
        "--option",
        "allow-import-from-derivation",
        "false",
        "--option",
        "builders",
        "",
    ]
    .map(String::from)
    .into()
}

/// Execute one addition after a durable, validated intent. Never creates GC roots.
pub async fn add(settings: &Settings, request: Invocation) -> Result<Value, String> {
    request.validate()?;
    let mut ledger = Ledger::open(settings)?;
    if let Some(receipt) = ledger.begin(&request)? {
        return Ok(receipt);
    }
    let mut args = vec![
        "store".into(),
        "add".into(),
        "--mode".into(),
        match request.mode {
            AddMode::Nar => "nar",
            AddMode::Flat => "flat",
        }
        .into(),
    ];
    args.extend(base_args(settings));
    args.push("--".into());
    args.push(request.path.to_str().ok_or("UTF-8 path required")?.into());
    let mut receipt = json!({"version":VERSION,"id":request.id,"scope":settings.scope,"host":settings.host,"store":settings.store,"reason":request.reason,
        "nix_executable":settings.nix,"argv":args,"retention":"unrooted","owned_roots":[],"objects":[],"coverage":"unknown","outcome":"unresolved"});
    match execute(settings, &request.cwd, &args).await {
        Err(ExecutionError::NotStarted(error)) => {
            receipt["outcome"] = json!("not-started");
            receipt["error"] = json!(error);
        }
        Err(ExecutionError::Unknown(error)) => {
            receipt["error"] = json!(error);
        }
        Ok(result) => {
            receipt["outcome"] = json!(if result.success {
                "succeeded"
            } else if result.code.is_none() {
                "interrupted"
            } else {
                "failed"
            });
            receipt["exit_code"] = json!(result.code);
            let path = result.stdout.text.trim();
            if result.success && result.stdout.omitted_bytes == 0 && valid_store_path(path) {
                receipt["objects"] = json!([{"path":path}]);
            }
            receipt["stdout"] = json!(result.stdout);
            receipt["stderr"] = json!(result.stderr);
            receipt["coverage"] = json!(if receipt["objects"]
                .as_array()
                .is_some_and(|objects| !objects.is_empty())
            {
                "returned-path-only"
            } else {
                "unknown"
            });
            // Save observed execution before any optional metadata work. A failure
            // to enrich/settle never erases the native outcome or returned path.
            ledger.executed(&request.id, &receipt)?;
            if let Some(path) = receipt["objects"][0]["path"].as_str().map(String::from) {
                let mut query = vec!["path-info".into(), "--json".into()];
                query.extend(base_args(settings));
                query.extend(["--".into(), path.clone()]);
                if let Ok(metadata) = execute(settings, &request.cwd, &query).await {
                    if metadata.success && metadata.stdout.omitted_bytes == 0 {
                        if let Ok(Value::Object(info)) =
                            serde_json::from_str::<Value>(&metadata.stdout.text)
                        {
                            if let Some(object) = info.get(&path).filter(|v| v.is_object()) {
                                let mut object = object.clone();
                                object["path"] = json!(path);
                                receipt["objects"] = json!([object]);
                                receipt["coverage"] = json!("returned-object-metadata");
                            }
                        }
                    }
                    receipt["metadata_capture"] = json!({"exit_code":metadata.code,"stdout":metadata.stdout,"stderr":metadata.stderr});
                }
                if receipt["coverage"] == "unknown" {
                    receipt["coverage"] = json!("returned-path-only");
                }
            }
        }
    }
    ledger.settle(&request.id, &receipt)?;
    Ok(receipt)
}

fn valid_store_path(path: &str) -> bool {
    let Some(name) = path.strip_prefix("/nix/store/") else {
        return false;
    };
    !name.contains('/')
        && name.len() > 33
        && name.as_bytes()[32] == b'-'
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-._?=".contains(&b))
}
