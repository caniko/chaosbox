//! Backend selection, `TypeDB` connection helpers, the pinned consumer reader, and the `db check` / `db migrate` lifecycle commands.

use super::*;

/// Pipeline store selection: `CHAOSBOX_DB_BACKEND=typedb` publishes through
/// `TypeDB`; anything else runs the disposable in-memory store. Lifecycle
/// and consumer commands always target `TypeDB`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Backend {
    Memory,
    Typedb,
}

/// Resolve the pipeline store from the environment.
pub(super) fn backend() -> Backend {
    if std::env::var("CHAOSBOX_DB_BACKEND").as_deref() == Ok("typedb") {
        Backend::Typedb
    } else {
        Backend::Memory
    }
}

/// Live spend of one `run`, recorded where every exit path can read it back.
///
/// The `usage:` line the batching caller budgets against is emitted once, by
/// the command arm, so a run that fails after dispatching requests still
/// reports what it spent instead of handing the next repository a fresh
/// allowance.
#[derive(Default)]
pub(super) struct RunSpend {
    pub(super) requests: std::sync::atomic::AtomicU32,
    pub(super) tokens: std::sync::atomic::AtomicU64,
}

/// `TypeDB` connection from the environment. The password arrives via a
/// credential file (never a value, flag, or log); only the file path
/// appears in diagnostics.
pub(super) fn typedb_config_from_env() -> Result<TypeDbConfig, String> {
    let password_file = std::env::var("CHAOSBOX_TYPEDB_PASSWORD_FILE")
        .map_err(|_| "CHAOSBOX_TYPEDB_PASSWORD_FILE unset".to_owned())?;
    let password =
        std::fs::read_to_string(&password_file).map_err(|e| format!("read password file: {e}"))?;
    Ok(TypeDbConfig {
        address: std::env::var("CHAOSBOX_TYPEDB_ADDR").unwrap_or_else(|_| "127.0.0.1:1729".into()),
        username: std::env::var("CHAOSBOX_TYPEDB_USER").unwrap_or_else(|_| "admin".into()),
        password: password.trim().to_owned(),
        database: std::env::var("CHAOSBOX_TYPEDB_DATABASE").unwrap_or_else(|_| "chaosbox".into()),
    })
}

/// Live publication chain for one repo: (expected predecessor, starting
/// generation) for a fresh process. Missing database or no active build
/// means a fresh chain; anything else is a hard error, never a guess.
pub(super) async fn typedb_publication_chain(
    config: &TypeDbConfig,
    repo: &str,
) -> Result<(Option<String>, u64), String> {
    let mut reader = TypeDbReader::new(config.clone());
    Box::pin(reader.connect())
        .await
        .map_err(|e| format!("typedb connect: {e}"))?;
    let active = Box::pin(reader.active_build(repo))
        .await
        .map_err(|e| format!("typedb active build: {e}"))?;
    chain_publication(active.map(|b| (b.build_id, b.generation)))
        .map_err(|e| format!("publication chain: {e}"))
}

/// Whether the active build still publishes relations.
///
/// Returns `Ok(None)` when the store definitively has no active build for
/// the repository (nothing consumers could lose), `Ok(Some(_))` when the
/// answer is known, and `Err(())` when it cannot be determined.
///
/// The in-process staging store answers for the memory backend and for a
/// same-process republish; a fresh `TypeDB` process has empty staging by
/// design, so it probes the pinned active build through the query reader
/// instead. Publication state is never inferred from an empty staging area:
/// `Err` means "cannot tell" (unreachable backend, failed query), and
/// callers must then leave the active build alone rather than risk
/// replacing a relation-bearing one. A successful query that simply finds
/// no build is `Ok(None)`, not an error: a first capture-only publish is
/// what lets any build exist at all.
pub(super) async fn active_publishes_relations<S: chaosbox_store::Store>(
    store: &S,
    repo: &str,
) -> Result<Option<bool>, ()> {
    if let Some(build) = store.active(repo) {
        return Ok(Some(!build.edges.is_empty()));
    }
    if backend() != Backend::Typedb {
        return Ok(Some(false));
    }
    let config = typedb_config_from_env().map_err(|_| ())?;
    let mut handle = TypeDbReader::new(config);
    Box::pin(handle.connect()).await.map_err(|_| ())?;
    let build = Box::pin(handle.active_build(repo)).await.map_err(|_| ())?;
    let Some(build) = build else {
        return Ok(None);
    };
    // Presence is all the gate asks: one row answers it without pulling the
    // whole relation set out of the store.
    let relations = Box::pin(handle.build_relationships(&build.build_id, 1))
        .await
        .map_err(|_| ())?;
    Ok(Some(!relations.is_empty()))
}

/// Consumer read surface over the live backend: a pinned `TypeDB` reader.
/// Credentials stay behind `connect`; MCP callers only see the closed
/// read surface. Query methods deref through to the pinned reader.
pub(super) struct AnyReader(GraphReader<TypeDbReader>);

impl AnyReader {
    /// Connect and pin the active build for `repo` on `TypeDB`.
    pub(super) async fn connect(repo: &str) -> Result<Self, PipelineError> {
        let config = typedb_config_from_env().map_err(PipelineError::Consumer)?;
        let mut handle = TypeDbReader::new(config);
        Box::pin(handle.connect())
            .await
            .map_err(|e| PipelineError::Consumer(format!("typedb connect: {e}")))?;
        Ok(Self(GraphReader::pinned(handle, repo).await?))
    }

    /// Pinned active build id for status responses.
    pub(super) fn build_id(&self) -> &str {
        &self.0.build_id
    }

    /// Pinned generation for status responses.
    pub(super) fn generation(&self) -> i64 {
        self.0.generation
    }

    /// Pinned build status for status responses.
    pub(super) fn status(&self) -> &str {
        &self.0.status
    }

    /// Pinned snapshot ids (freshness fingerprint) for status responses.
    pub(super) fn snapshots(&self) -> &[String] {
        &self.0.snapshots
    }
}

impl std::ops::Deref for AnyReader {
    type Target = GraphReader<TypeDbReader>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// TypeDB-backed readiness: connectivity + schema probe + active build.
/// Reports contract v2; exit 0 ready, 2 pending, 1 otherwise.
pub(super) async fn db_check_typedb(repo: &str) -> LifecycleReport {
    let config = match typedb_config_from_env() {
        Ok(c) => c,
        Err(e) => return LifecycleReport::error("db check", &e),
    };
    let mut reader = TypeDbReader::new(config);
    if let Err(e) = Box::pin(reader.connect()).await {
        // A missing database is the normal pre-migration state, not a
        // failure; anything else is an operational error.
        if e.to_string().contains("not found") {
            return LifecycleReport::pending("db check", "database not present");
        }
        return LifecycleReport::error("db check", &format!("typedb connect: {e}"));
    }
    match Box::pin(reader.probe()).await {
        Err(e) => LifecycleReport::error("db check", &format!("probe: {e}")),
        // No marker type: the packaged schema has not applied yet.
        Ok(false) => LifecycleReport::pending("db check", "migrations not applied"),
        Ok(true) => match Box::pin(reader.active_build(repo)).await {
            Err(e) => LifecycleReport::error("db check", &format!("active build: {e}")),
            Ok(None) => LifecycleReport::pending("db check", "no active build for repo"),
            Ok(Some(b)) => LifecycleReport::check_ready(serde_json::json!({
                "repo": repo, "active_build": b.build_id, "generation": b.generation,
                "status": b.status, "schema_assets": "packaged",
            })),
        },
    }
}

/// `TypeDB` migration: ensure the database and apply the packaged schema
/// idempotently through the driver, then verify schema readiness itself.
pub(super) async fn run_migrate_typedb() -> Result<LifecycleReport, String> {
    let config = typedb_config_from_env()?;
    let mut store = TypeDbStore::new(config);
    Box::pin(store.migrate())
        .await
        .map_err(|e| format!("typedb migrate: {e}"))?;
    Ok(LifecycleReport {
        contract_version: 2,
        backend: "typedb".into(),
        operation: "db migrate".into(),
        status: "ready".into(),
        schema_version: chaosbox_typedb::SCHEMA_VERSION,
        pinned: chaosbox_typedb::TYPEDB_PINNED.into(),
        detail: serde_json::json!({"applied": true}),
    })
}

pub(super) fn consumer_err(op: &str, e: impl std::fmt::Display) -> i32 {
    let report = LifecycleReport::error(op, &e.to_string());
    println!("{}", serde_json::to_string(&report).unwrap());
    eprintln!("{op} failed: {e}");
    1
}
