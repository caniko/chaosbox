//! Backend selection, PostgreSQL connection helpers, the pinned reader, and database commands.

use super::*;

/// Pipeline store selection: `CHAOSBOX_DB_BACKEND=postgres` publishes through
/// PostgreSQL; anything else runs the disposable in-memory store. Lifecycle
/// and consumer commands always target PostgreSQL.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Backend {
    Memory,
    Postgres,
}

/// Resolve the pipeline store from the environment.
pub(super) fn backend() -> Backend {
    if std::env::var("CHAOSBOX_DB_BACKEND").as_deref() == Ok("postgres") {
        Backend::Postgres
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

/// PostgreSQL connection from `CHAOSBOX_DATABASE_URL`: a unix socket or
/// loopback address only. The string is never printed (it may carry a
/// password).
pub(super) fn pg_config_from_env() -> Result<PgConfig, String> {
    PgConfig::from_env()
}

/// Live publication chain for one repo: (expected predecessor, starting
/// generation) for a fresh process. Missing database or no active build
/// means a fresh chain; anything else is a hard error, never a guess.
pub(super) async fn pg_publication_chain(
    config: &PgConfig,
    repo: &str,
) -> Result<(Option<String>, u64), String> {
    let mut reader = PgReader::new(config.clone());
    Box::pin(reader.connect())
        .await
        .map_err(|e| format!("postgres connect: {e}"))?;
    let active = Box::pin(reader.active_build(repo))
        .await
        .map_err(|e| format!("postgres active build: {e}"))?;
    chain_publication(active.map(|b| (b.build_id, b.generation)))
        .map_err(|e| format!("publication chain: {e}"))
}

/// Whether the active build publishes decision-backed (or legacy) relations.
///
/// Returns `Ok(None)` when the store definitively has no active build for
/// the repository (nothing consumers could lose), `Ok(Some(_))` when the
/// answer is known, and `Err(msg)` when it cannot be determined.
///
/// The in-process staging store answers for the memory backend and for a
/// same-process republish; a fresh PostgreSQL process has empty staging by
/// design, so it probes the pinned active build through the query reader
/// instead. Publication state is never inferred from an empty staging area:
/// `Err` means "cannot tell" (unreachable backend, failed query), and
/// callers must fail rather than defer — a read failure is operational, not
/// a kept query view. A successful query that simply finds
/// no build is `Ok(None)`, not an error: a first capture-only publish is
/// what lets any build exist at all.
pub(super) async fn active_publishes_relations<S: chaosbox_store::Store>(
    store: &S,
    repo: &str,
) -> Result<Option<bool>, String> {
    if let Some(build) = store.active(repo) {
        return Ok(Some(
            build
                .coverage
                .as_ref()
                .map_or(!build.edges.is_empty(), |c| c.decision_relations > 0),
        ));
    }
    if backend() != Backend::Postgres {
        return Ok(Some(false));
    }
    let config = pg_config_from_env().map_err(|e| format!("postgres config: {e}"))?;
    let mut handle = PgReader::new(config);
    Box::pin(handle.connect())
        .await
        .map_err(|e| format!("postgres connect: {e}"))?;
    let build = Box::pin(handle.active_build(repo))
        .await
        .map_err(|e| format!("postgres active build: {e}"))?;
    let Some(build) = build else {
        return Ok(None);
    };
    if let Some(coverage) = build.coverage {
        return Ok(Some(coverage.decision_relations > 0));
    }
    // Presence is all the gate asks: one row answers it without pulling the
    // whole relation set out of the store.
    let relations = Box::pin(handle.build_relationships(&build.build_id, 1))
        .await
        .map_err(|e| format!("postgres relationships: {e}"))?;
    Ok(Some(!relations.is_empty()))
}

/// Consumer read surface over the live backend: a pinned PostgreSQL reader.
/// Credentials stay behind `connect`; MCP callers only see the closed
/// read surface. Query methods deref through to the pinned reader.
pub(super) struct AnyReader(GraphReader<PgReader>);

pub(super) const NEIGHBORHOOD_DEFAULT_LIMIT: usize = 20;

impl AnyReader {
    async fn handle() -> Result<PgReader, PipelineError> {
        Self::handle_cached(&mut None).await
    }

    /// Reuse a live session only, after rereading the operator configuration.
    /// The caller owns eviction and must not retain this slot after failure.
    pub(super) async fn handle_cached(
        slot: &mut Option<PgReader>,
    ) -> Result<PgReader, PipelineError> {
        let config = pg_config_from_env().map_err(PipelineError::Consumer)?;
        if let Some(handle) = slot
            .as_ref()
            .filter(|handle| handle.matches_config(&config) && !handle.is_closed())
        {
            return Ok(handle.clone());
        }
        *slot = None;
        let mut handle = PgReader::new(config);
        Box::pin(handle.connect())
            .await
            .map_err(|e| PipelineError::Consumer(format!("postgres connect: {e}")))?;
        *slot = Some(handle.clone());
        Ok(handle)
    }

    /// Connect and pin the active build for `repo` on PostgreSQL.
    pub(super) async fn connect(repo: &str) -> Result<Self, PipelineError> {
        Self::connect_at(repo, None).await
    }

    /// Validate selection before acquiring credentials, then pin exactly once.
    pub(super) async fn connect_at(
        repo: &str,
        build_id: Option<&str>,
    ) -> Result<Self, PipelineError> {
        if let Some(build_id) = build_id {
            chaosbox::validate_build_id(build_id)?;
        }
        Self::pin(Self::handle().await?, repo, build_id).await
    }

    /// Pin a fresh publication on an existing transport, never a previous reader.
    pub(super) async fn pin(
        handle: PgReader,
        repo: &str,
        build_id: Option<&str>,
    ) -> Result<Self, PipelineError> {
        Ok(Self(GraphReader::selected(handle, repo, build_id).await?))
    }

    /// Acquire credentials only after shared preparation has validated the query.
    pub(super) async fn neighborhood_page(
        query: chaosbox::PreparedNeighborhood,
    ) -> Result<chaosbox::NeighborhoodPage, PipelineError> {
        query.read(Self::handle().await?).await
    }

    /// Start the whole path budget before polling credential/backend acquisition.
    pub(super) async fn path_query(
        query: chaosbox::PreparedPath,
    ) -> Result<serde_json::Value, PipelineError> {
        query.read(Self::handle()).await
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
    type Target = GraphReader<PgReader>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// PostgreSQL-backed readiness: connectivity + schema probe + active build.
/// Reports contract v2; exit 0 ready, 2 pending, 1 otherwise.
pub(super) async fn db_check_pg(repo: &str) -> LifecycleReport {
    let config = match pg_config_from_env() {
        Ok(c) => c,
        Err(e) => return LifecycleReport::error("db check", &e),
    };
    let mut reader = PgReader::new(config);
    if let Err(e) = Box::pin(reader.connect()).await {
        // A missing database is the normal pre-migration state, not a
        // failure; anything else is an operational error.
        if e.to_string().contains("not found") {
            return LifecycleReport::pending("db check", "database not present");
        }
        return LifecycleReport::error("db check", &format!("postgres connect: {e}"));
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

/// PostgreSQL migration: apply the packaged schema to the existing database
/// idempotently through the driver, then verify schema readiness itself.
pub(super) async fn run_migrate_pg() -> Result<LifecycleReport, String> {
    let config = pg_config_from_env()?;
    let mut store = PgStore::new(config);
    Box::pin(store.migrate())
        .await
        .map_err(|e| format!("postgres migrate: {e}"))?;
    Ok(LifecycleReport {
        contract_version: 2,
        backend: "postgres".into(),
        operation: "db migrate".into(),
        status: "ready".into(),
        schema_version: chaosbox_pg::SCHEMA_VERSION,
        pinned: chaosbox_pg::POSTGRES_PINNED.into(),
        detail: serde_json::json!({"applied": true}),
    })
}

pub(super) fn consumer_err(op: &str, e: impl std::fmt::Display) -> i32 {
    let report = LifecycleReport::error(op, &e.to_string());
    println!("{}", serde_json::to_string(&report).unwrap());
    eprintln!("{op} failed: {e}");
    1
}

/// Inspect retained headers and recorded counts through the read-only route.
pub(super) async fn run_inventory_pg(
    repo: String,
    limit: usize,
    after: Option<String>,
) -> Result<LifecycleReport, String> {
    use chaosbox_pg::reader::{InventoryQuery, INVENTORY_TIMEOUT};
    let query = InventoryQuery::new(repo, limit, after).map_err(|error| error.to_string())?;
    let page = tokio::time::timeout(INVENTORY_TIMEOUT, async {
        let mut reader = PgReader::new(pg_config_from_env()?);
        Box::pin(reader.connect())
            .await
            .map_err(|error| error.to_string())?;
        reader
            .inventory(&query)
            .await
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "build inventory deadline exceeded")??;
    Ok(LifecycleReport {
        contract_version: 2,
        backend: "postgres".into(),
        operation: "db inventory".into(),
        status: "ready".into(),
        schema_version: chaosbox_pg::SCHEMA_VERSION,
        pinned: chaosbox_pg::POSTGRES_PINNED.into(),
        detail: serde_json::json!({"contract":"chaosbox.build-inventory.v1", "page":page}),
    })
}
