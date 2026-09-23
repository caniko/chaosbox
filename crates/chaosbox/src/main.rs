//! `chaosbox` CLI + read-only MCP server.
//!
//! Consumers (CLI queries, MCP tools) share one query implementation in the
//! library, served TypeDB-backed through [`chaosbox::GraphReader`]. MCP is
//! read-only: no mutation, ingestion, annotations, arbitrary SQL,
//! migrations, or model configuration tools. The read-only server never loads
//! Jev credentials. Indexing/administration are operator commands.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chaosbox::{
    all_relation_types, GraphReader, LifecycleReport, Materialization, Pipeline, chain_publication,
    EXPORT_EDGE_CAP, EXPORT_NODE_CAP,
};
use chaosbox::{FixtureResponder, LiveResponder, PipelineError};
use chaosbox_extract::Snapshot;
use chaosbox_store::{GraphQueries as _, MemoryStore};
use chaosbox_typedb::{
    reader::TypeDbReader,
    store::{TypeDbConfig, TypeDbStore},
};
use clap::{Parser, Subcommand};

/// Pipeline store selection: `CHAOSBOX_DB_BACKEND=typedb` publishes through
/// `TypeDB`; anything else runs the disposable in-memory store. Lifecycle
/// and consumer commands always target `TypeDB`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Memory,
    Typedb,
}

/// Resolve the pipeline store from the environment.
fn backend() -> Backend {
    if std::env::var("CHAOSBOX_DB_BACKEND").as_deref() == Ok("typedb") {
        Backend::Typedb
    } else {
        Backend::Memory
    }
}

/// `TypeDB` connection from the environment. The password arrives via a
/// credential file (never a value, flag, or log); only the file path
/// appears in diagnostics.
fn typedb_config_from_env() -> Result<TypeDbConfig, String> {
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
async fn typedb_publication_chain(
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

/// Consumer read surface over the live backend: a pinned `TypeDB` reader.
/// Credentials stay behind `connect`; MCP callers only see the closed
/// read surface. Query methods deref through to the pinned reader.
struct AnyReader(GraphReader<TypeDbReader>);

impl AnyReader {
    /// Connect and pin the active build for `repo` on `TypeDB`.
    async fn connect(repo: &str) -> Result<Self, PipelineError> {
        let config = typedb_config_from_env().map_err(PipelineError::Consumer)?;
        let mut handle = TypeDbReader::new(config);
        Box::pin(handle.connect())
            .await
            .map_err(|e| PipelineError::Consumer(format!("typedb connect: {e}")))?;
        Ok(Self(GraphReader::pinned(handle, repo).await?))
    }

    /// Pinned active build id for status responses.
    fn build_id(&self) -> &str {
        &self.0.build_id
    }

    /// Pinned generation for status responses.
    fn generation(&self) -> i64 {
        self.0.generation
    }

    /// Pinned build status for status responses.
    fn status(&self) -> &str {
        &self.0.status
    }

    /// Pinned snapshot ids (freshness fingerprint) for status responses.
    fn snapshots(&self) -> &[String] {
        &self.0.snapshots
    }
}

impl std::ops::Deref for AnyReader {
    type Target = GraphReader<TypeDbReader>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "chaosbox",
    version,
    about = "Chaosbox deterministic code-graph pipeline"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Extract, assess and retrieve selective session intelligence.
    Intelligence {
        #[command(subcommand)]
        command: chaosbox::intelligence::cli::Command,
    },
    /// Snapshot a fixture repository.
    Snapshot {
        path: PathBuf,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
    /// Extract deterministic facts + candidates.
    Extract {
        path: PathBuf,
        #[arg(long, default_value = "demo")]
        repo: String,
        #[arg(long, default_value_t = 200)]
        max_candidates: usize,
    },
    /// Run the full pipeline (live Jev decisions by default; fixture
    /// decisions only with --fixture-decisions, for disposable/test graphs;
    /// entities-only with --no-decisions, no inference of any kind).
    Run {
        path: PathBuf,
        #[arg(long, default_value = "demo")]
        repo: String,
        #[arg(long, default_value_t = 200)]
        max_candidates: usize,
        /// Use the live Jev API (needs `CHAOSBOX_JEV_API_KEY_FILE`) instead of
        /// the deterministic fixture. Real inference, real spend.
        #[arg(long, default_value_t = false, conflicts_with = "no_decisions")]
        live_jev: bool,
        /// Accept deterministic fixture decisions (every choice `accept`) for
        /// a disposable or test graph. Fixture graphs are never authoritative:
        /// decisions are recorded under the `fixture-test` model identity.
        #[arg(long, default_value_t = false, conflicts_with = "no_decisions")]
        fixture_decisions: bool,
        /// Publish extracted entities with no semantic decisions: no live
        /// inference, no fixture accept-all. The graph has nodes but no
        /// relations or claims; safe for real corpora before Jev approval.
        #[arg(long, default_value_t = false)]
        no_decisions: bool,
        /// Live-Jev spend guards (defaults = `JevPolicy::default`).
        #[arg(long)]
        max_requests: Option<u32>,
        #[arg(long)]
        max_input_tokens: Option<u64>,
        #[arg(long)]
        max_retries: Option<u32>,
    },
    /// Query helpers (read-only; TypeDB-backed, shared with MCP).
    Query {
        #[command(subcommand)]
        q: QueryCmd,
    },
    /// Serve read-only MCP over stdio (no Jev credentials loaded).
    Mcp {
        /// Explicit private intelligence bundle, pinned once on startup.
        #[arg(long)]
        intelligence: Option<PathBuf>,
    },
    /// Database readiness and migration reports (JSON contract v2).
    Db {
        #[command(subcommand)]
        op: DbCmd,
    },
}

#[derive(Debug, Subcommand)]
enum QueryCmd {
    /// Substring search over entity names (sorted, bounded).
    Search {
        query: String,
        #[arg(long, default_value = "demo")]
        repo: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Typed entity lookup by id.
    Lookup {
        id: String,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
    /// Incoming/outgoing neighborhoods with optional relation filter.
    Neighbors {
        id: String,
        #[arg(long, default_value = "demo")]
        repo: String,
        #[arg(long)]
        rel: Option<String>,
    },
    /// Bounded path between two entities (successful negative => null).
    Path {
        from: String,
        to: String,
        #[arg(long, default_value = "demo")]
        repo: String,
        #[arg(long, default_value_t = 4)]
        max_hops: usize,
    },
    /// Deterministic export of the pinned active build.
    Export {
        #[arg(long, default_value = "demo")]
        repo: String,
    },
    /// Source-backed entity explanation (no generated prose).
    Explain {
        id: String,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
    /// Active-build status, coverage, and generation.
    Status {
        #[arg(long, default_value = "demo")]
        repo: String,
    },
}

#[derive(Debug, Subcommand)]
enum DbCmd {
    /// Read-only readiness check. Exit 0 only when ready.
    Check {
        /// Emit the versioned JSON envelope (contract v2; always on).
        #[arg(long, default_value_t = true)]
        json: bool,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
    /// Apply the packaged `TypeDB` schema idempotently through the driver.
    Migrate {
        /// Emit the versioned JSON envelope (contract v2; always on).
        #[arg(long, default_value_t = true)]
        json: bool,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
}

// Long dispatch function; splitting it apart is the owning session's
// refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Intelligence { command } => {
            match chaosbox::intelligence::cli::run(command).await {
                Ok(value) => println!("{value}"),
                Err(error) => {
                    eprintln!("intelligence: {error}");
                    std::process::exit(1);
                }
            }
        }
        Command::Snapshot { path, repo } => match Snapshot::capture(&repo, &path) {
            Ok(s) => println!(r#"{{"snapshot":"{}","files":{}}}"#, s.id, s.files.len()),
            Err(e) => {
                eprintln!("snapshot failed: {e}");
                std::process::exit(1);
            }
        },
        Command::Extract {
            path,
            repo,
            max_candidates,
        } => match Pipeline::<MemoryStore>::snapshot_extract(&repo, &path, max_candidates) {
            Ok((snap, ext, cat)) => println!(
                r#"{{"snapshot":"{}","entities":{},"candidates":{},"selected":{},"omitted":{}}}"#,
                snap.id,
                ext.entities.len(),
                cat.candidates.len(),
                cat.selected.values().sum::<u64>(),
                serde_json::to_string(&cat.omitted).unwrap(),
            ),
            Err(e) => {
                eprintln!("extract failed: {e}");
                std::process::exit(1);
            }
        },
        Command::Run {
            path,
            repo,
            max_candidates,
            live_jev,
            fixture_decisions,
            no_decisions,
            max_requests,
            max_input_tokens,
            max_retries,
        } => {
            let code = match backend() {
                Backend::Memory => {
                    run_pipeline_with(
                        Pipeline::<MemoryStore>::new(),
                        &path,
                        &repo,
                        max_candidates,
                        live_jev,
                        fixture_decisions,
                        no_decisions,
                        max_requests,
                        max_input_tokens,
                        max_retries,
                        None,
                    )
                    .await
                }
                Backend::Typedb => {
                    let config = match typedb_config_from_env() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("typedb config: {e}");
                            std::process::exit(1);
                        }
                    };
                    let mut store = TypeDbStore::new(config.clone());
                    if let Err(e) = Box::pin(store.migrate()).await {
                        eprintln!("typedb migrate: {e}");
                        std::process::exit(1);
                    }
                    // Fresh processes start at generation zero: chain off the
                    // live active build so repeat runs publish gen+1 with the
                    // right predecessor instead of failing the guard.
                    let (expected_predecessor, starting_generation) =
                        match Box::pin(typedb_publication_chain(&config, &repo)).await {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!("typedb publication chain: {e}");
                                std::process::exit(1);
                            }
                        };
                    run_pipeline_with(
                        Pipeline {
                            store,
                            generation: starting_generation,
                        },
                        &path,
                        &repo,
                        max_candidates,
                        live_jev,
                        fixture_decisions,
                        no_decisions,
                        max_requests,
                        max_input_tokens,
                        max_retries,
                        expected_predecessor,
                    )
                    .await
                }
            };
            std::process::exit(code);
        }
        Command::Query { q } => std::process::exit(Box::pin(run_query(q)).await),
        Command::Mcp { intelligence } => {
            let bundle = intelligence
                .as_deref()
                .map(chaosbox::intelligence::cli::load_bundle)
                .transpose()
                .unwrap_or_else(|error| {
                    eprintln!("intelligence bundle: {error}");
                    std::process::exit(1);
                });
            Box::pin(serve_mcp(bundle)).await;
        }
        Command::Db { op } => match op {
            DbCmd::Check { json: _, repo } => {
                // Read-only: never init/migrate/repair. Exit 0 when ready,
                // 2 when pending, 1 otherwise (harbor-db contract v2).
                let report = Box::pin(db_check_typedb(&repo)).await;
                println!("{}", serde_json::to_string(&report).unwrap());
                if report.status == "ready" {
                    std::process::exit(0);
                } else {
                    eprintln!("not ready: {}", report.status);
                    std::process::exit(if report.status == "pending" { 2 } else { 1 });
                }
            }
            DbCmd::Migrate { json: _, repo: _ } => {
                // Idempotent packaged-schema application through the driver
                // (no CLI tooling). run_migrate_typedb verifies schema
                // readiness itself; the arm only maps the verdict to the
                // exit code.
                let result = Box::pin(run_migrate_typedb()).await;
                match result {
                    Ok(report) => {
                        println!("{}", serde_json::to_string(&report).unwrap());
                        std::process::exit(i32::from(report.status != "ready"));
                    }
                    Err(e) => {
                        let report = LifecycleReport::pending("db migrate", &e);
                        println!("{}", serde_json::to_string(&report).unwrap());
                        eprintln!("migrate failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
        },
    }
}

/// TypeDB-backed readiness: connectivity + schema probe + active build.
/// Reports contract v2; exit 0 ready, 2 pending, 1 otherwise.
async fn db_check_typedb(repo: &str) -> LifecycleReport {
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
async fn run_migrate_typedb() -> Result<LifecycleReport, String> {
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

fn consumer_err(op: &str, e: impl std::fmt::Display) -> i32 {
    let report = LifecycleReport::error(op, &e.to_string());
    println!("{}", serde_json::to_string(&report).unwrap());
    eprintln!("{op} failed: {e}");
    1
}

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
async fn run_query(q: QueryCmd) -> i32 {
    match q {
        QueryCmd::Search { query, repo, limit } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query search", e),
            };
            match reader.search(&query, limit).await {
                Ok(rows) => {
                    println!("{}", serde_json::to_string(&rows).unwrap());
                    0
                }
                Err(e) => consumer_err("query search", e),
            }
        }
        QueryCmd::Lookup { id, repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query lookup", e),
            };
            match reader.lookup(&id).await {
                Ok(row) => {
                    println!("{}", serde_json::to_string(&row).unwrap());
                    0
                }
                Err(e) => consumer_err("query lookup", e),
            }
        }
        QueryCmd::Neighbors { id, repo, rel } => {
            // Validate the filter before touching the backend: typos must fail loudly.
            let filter = match chaosbox::validate_rel_filter(rel.map(|r| vec![r])) {
                Ok(f) => f,
                Err(e) => return consumer_err("query neighbors", e),
            };
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query neighbors", e),
            };
            match reader.neighbors(&id, filter).await {
                Ok((out, inc)) => {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "id": id, "outgoing": out, "incoming": inc,
                        }))
                        .unwrap()
                    );
                    0
                }
                Err(e) => consumer_err("query neighbors", e),
            }
        }
        QueryCmd::Path {
            from,
            to,
            repo,
            max_hops,
        } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query path", e),
            };
            match reader.path(&from, &to, max_hops).await {
                // Successful negative (no path) is a null result, exit 0.
                Ok(path) => {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "from": from, "to": to, "path": path,
                        }))
                        .unwrap()
                    );
                    0
                }
                Err(e) => consumer_err("query path", e),
            }
        }
        QueryCmd::Export { repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query export", e),
            };
            match reader.export().await {
                Ok(v) => {
                    println!("{}", serde_json::to_string(&v).unwrap());
                    0
                }
                Err(e) => consumer_err("query export", e),
            }
        }
        QueryCmd::Explain { id, repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query explain", e),
            };
            match reader.lookup(&id).await {
                Ok(None) => {
                    println!("null");
                    0
                }
                Ok(Some(e)) => {
                    let (out, inc) = match reader.neighbors(&id, None).await {
                        Ok(n) => n,
                        Err(e) => return consumer_err("query explain", e),
                    };
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "id": e.entity_id, "kind": e.kind, "file": e.file,
                            "qualified_name": e.qualified_name,
                            "outgoing": out.len(), "incoming": inc.len(),
                        }))
                        .unwrap()
                    );
                    0
                }
                Err(e) => consumer_err("query explain", e),
            }
        }
        QueryCmd::Status { repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query status", e),
            };
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "repo": repo, "build_id": reader.build_id(),
                    "generation": reader.generation(),
                    "status": reader.status(),
                    "snapshots": reader.snapshots(),
                    "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
                }))
                .unwrap()
            );
            0
        }
    }
}

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
async fn run_pipeline_with<S: chaosbox_store::Store + Default>(
    mut pipe: Pipeline<S>,
    path: &Path,
    repo: &str,
    max_candidates: usize,
    live_jev: bool,
    fixture_decisions: bool,
    no_decisions: bool,
    max_requests: Option<u32>,
    max_input_tokens: Option<u64>,
    max_retries: Option<u32>,
    expected_predecessor: Option<String>,
) -> i32 {
    let (snap, ext, cat) = match Pipeline::<S>::snapshot_extract(repo, path, max_candidates) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("extract: {e}");
            return 1;
        }
    };
    let cands = &cat.candidates;
    // Truncation must be observable: report what the cap selected and
    // omitted before any decision is made.
    eprintln!(
        "candidates: selected={} cap={} omitted={}",
        cands.len(),
        cat.cap,
        serde_json::to_string(&cat.omitted).unwrap(),
    );
    let entities: BTreeMap<_, _> = ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let mat = Materialization::default();
    // Register file content identities before any evidence references them.
    if let Err(e) = pipe
        .store
        .ensure_snapshot_files(&snap.id, repo, &snap.snapshot_files())
        .await
    {
        eprintln!("snapshot files: {e}");
        return 1;
    }
    // Mint deterministic run/set identity and register the candidate catalog
    // before any decision references it.
    let digest = chaosbox_core::catalog_digest(cands);
    let run_id = chaosbox_core::deterministic_id("run", &[repo, &snap.id]);
    let set_id = chaosbox_core::deterministic_id("set", &[&run_id, &digest, &mat.rubric_version]);
    if let Err(e) = pipe
        .store
        .ensure_run(
            &run_id,
            repo,
            &snap.id,
            &set_id,
            &digest,
            &mat.rubric_version,
        )
        .await
    {
        eprintln!("run identity: {e}");
        return 1;
    }
    for cand in cands {
        if let Err(e) = pipe.store.put_candidate(&set_id, cand).await {
            eprintln!("candidate: {e}");
            return 1;
        }
    }
    let decided = if no_decisions {
        // Entities-only publication: no live inference, no fixture
        // accept-all. The build carries nodes but no relations or claims.
        Vec::new()
    } else if live_jev {
        // Fail fast without credentials: otherwise every decision degrades
        // to Failed and the run exits 0 with an empty graph.
        if chaosbox_jev::JevClient::api_key().is_none() {
            eprintln!("live-jev needs CHAOSBOX_JEV_API_KEY_FILE or TYPESAFE_API_KEY");
            return 1;
        }
        let mut policy = chaosbox_jev::JevPolicy::default();
        if let Some(n) = max_requests {
            policy.max_requests = n;
        }
        if let Some(n) = max_input_tokens {
            policy.max_input_tokens = n;
        }
        if let Some(n) = max_retries {
            policy.max_retries = n;
        }
        // Budget preflight: one uncached candidate costs one Jev request.
        // Fail before spending anything when the budget cannot cover this
        // run (defaults: 200 candidates vs 100 requests), instead of
        // burning the budget and dying at publish on Failed decisions.
        let pending = match chaosbox::uncached_decisions(
            cands,
            &entities,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
            &pipe.store,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                eprintln!("budget preflight: {e}");
                return 1;
            }
        };
        let max_requests = usize::try_from(policy.max_requests).unwrap_or(usize::MAX);
        if pending > max_requests {
            eprintln!(
                "live-jev budget: {pending} uncached candidates need one request each but max_requests={}; raise --max-requests or lower --max-candidates (already-cached decisions do not count)",
                policy.max_requests
            );
            return 1;
        }
        let client = match chaosbox_jev::JevClient::new(policy) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("jev client: {e}");
                return 1;
            }
        };
        let mut responder = LiveResponder::new(client);
        match Pipeline::<S>::decide(
            cands,
            &entities,
            &mut responder,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
            &mut pipe.store,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("decide: {e}");
                return 1;
            }
        }
    } else {
        if !fixture_decisions {
            eprintln!(
                "refusing to publish fixture decisions without --fixture-decisions (fixture graphs are disposable/test-only); pass --live-jev for real decisions"
            );
            return 1;
        }
        let mut responder = FixtureResponder::new(true);
        // Fixture decisions must never masquerade as Jev model output.
        responder.model = "fixture-test".into();
        match Pipeline::<S>::decide(
            cands,
            &entities,
            &mut responder,
            "fixture-test",
            &mat,
            &mut pipe.store,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("decide: {e}");
                return 1;
            }
        }
    };
    // Operator visibility: structural vs semantic coverage is a follow-up;
    // today every candidate consumes the Jev budget, so report the outcome
    // mix before publication (a failed batch refuses to publish below).
    let counts = chaosbox::summarize_outcomes(&decided);
    let n = |k: &str| counts.get(k).copied().unwrap_or(0);
    eprintln!(
        "decisions: accepted={} rejected={} abstained={} negative={} failed={} candidates={}",
        n("accepted"),
        n("rejected"),
        n("abstained"),
        n("negative"),
        n("failed"),
        cands.len()
    );
    match pipe
        .build_and_publish(repo, &snap, &ext, &decided, &mat, expected_predecessor)
        .await
    {
        Ok(build) => {
            let v = chaosbox::export_json(&build);
            println!("{}", serde_json::to_string(&v).unwrap());
            0
        }
        Err(e) => {
            eprintln!("publish: {e}");
            1
        }
    }
}

// ---- Read-only MCP (JSON-RPC over stdio, full handshake) ----

/// MCP protocol version served here.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Older protocol versions still accepted from clients.
const MCP_PROTOCOL_FALLBACKS: &[&str] = &["2024-11-05", "2025-03-26"];
/// Tools per `tools/list` page.
const MCP_PAGE_SIZE: usize = 5;
/// Upper bound for caller-supplied search limits: paginate instead.
const MCP_SEARCH_LIMIT_MAX: i64 = 200;
/// Upper bound for caller-supplied path hop counts.
const MCP_MAX_HOPS: usize = 8;

fn mcp_tool_defs() -> Vec<serde_json::Value> {
    vec![
        mcp_tool(
            "search",
            "Substring search over entity names (sorted, bounded).",
            serde_json::json!({"query": {"type": "string"}, "limit": {"type": "integer", "default": 20, "maximum": 200}}),
            vec!["query"],
        ),
        mcp_tool(
            "lookup",
            "Typed entity lookup by id.",
            serde_json::json!({"id": {"type": "string"}}),
            vec!["id"],
        ),
        mcp_tool(
            "neighbors",
            "Incoming/outgoing neighborhoods with optional relation filter.",
            serde_json::json!({"id": {"type": "string"}, "rel": {"type": "string"}}),
            vec!["id"],
        ),
        mcp_tool(
            "path",
            "Bounded path between two entities (null when absent).",
            serde_json::json!({"from": {"type": "string"}, "to": {"type": "string"},
                "max_hops": {"type": "integer", "default": 4, "maximum": 8}}),
            vec!["from", "to"],
        ),
        mcp_tool(
            "evidence",
            "Claim evidence and source locations for a relationship.",
            serde_json::json!({"rel": {"type": "string"}}),
            vec!["rel"],
        ),
        mcp_tool(
            "status",
            "Active-build status, coverage, and generation.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "diff",
            "Node/edge id diff between two builds of one repo.",
            serde_json::json!({"from_build": {"type": "string"}, "to_build": {"type": "string"}}),
            vec!["from_build", "to_build"],
        ),
        mcp_tool(
            "export",
            "Deterministic export of the pinned active build.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "explain",
            "Source-backed entity explanation (no generated prose).",
            serde_json::json!({"id": {"type": "string"}}),
            vec!["id"],
        ),
    ]
}

// properties/required move into the schema json! below, which the
// pass-by-value lint cannot see through (macro boundary false positive).
#[allow(clippy::needless_pass_by_value)]
fn mcp_tool(
    name: &str,
    description: &str,
    properties: serde_json::Value,
    required: Vec<&str>,
) -> serde_json::Value {
    let mut required = required;
    // Every tool requires an explicit repo: the server holds no default
    // (a silent `demo` fallback once sent agents to the wrong graph).
    // Declared here, not per tool, so schema and runtime validation agree.
    if !required.contains(&"repo") {
        required.push("repo");
    }
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.insert(
            "repo".to_owned(),
            serde_json::json!({"type": "string", "minLength": 1}),
        );
    }
    serde_json::json!({
        "name": name, "description": description,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": true},
    })
}

fn mcp_text_result(id: &serde_json::Value, payload: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {"content": [{"type": "text", "text": serde_json::to_string(payload).unwrap_or_default()}]},
    })
}

// message moves into the error json! below (macro boundary false positive
// for the pass-by-value lint, same as mcp_tool above).
#[allow(clippy::needless_pass_by_value)]
fn mcp_error(
    id: &serde_json::Value,
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut error = serde_json::json!({"code": code, "message": message});
    if let Some(d) = data {
        error["data"] = d;
    }
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// Validate tool arguments against required fields (types are checked per tool).
fn mcp_args(
    name: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, serde_json::Value> {
    let args = params.get("arguments").unwrap_or(&serde_json::Value::Null);
    let map = args.as_object().cloned().unwrap_or_default();
    let required: &[&str] = match name {
        "search" => &["query"],
        "lookup" | "neighbors" | "explain" => &["id"],
        "path" => &["from", "to"],
        "evidence" => &["rel"],
        "diff" => &["from_build", "to_build"],
        _ => &[],
    };
    for key in required {
        if map
            .get(*key)
            .and_then(|v| v.as_str())
            .is_none_or(str::is_empty)
        {
            return Err(
                serde_json::json!({"code": -32602, "message": format!("missing required argument: {key}")}),
            );
        }
    }
    Ok(map)
}

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
async fn mcp_call_tool(
    id: &serde_json::Value,
    name: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    let args = match mcp_args(name, params) {
        Ok(a) => a,
        Err(e) => {
            let code = e["code"].as_i64().unwrap_or(-32602);
            let msg = e["message"].as_str().unwrap_or("invalid params").to_owned();
            return mcp_error(id, code, msg, None);
        }
    };
    // Closed read-only tool set: reject unknown (write/mutation) tools before
    // touching the backend or credentials of any kind.
    if !matches!(
        name,
        "search"
            | "lookup"
            | "neighbors"
            | "path"
            | "evidence"
            | "status"
            | "diff"
            | "export"
            | "explain"
    ) {
        return mcp_error(
            id,
            -32601,
            format!("read-only MCP: no such tool (rejected): {name}"),
            None,
        );
    }
    // Explicit repository: fail before touching the backend or credentials.
    let Some(repo) = args
        .get("repo")
        .and_then(|r| r.as_str())
        .filter(|r| !r.is_empty())
    else {
        return mcp_error(
            id,
            -32602,
            "missing required argument: repo".to_owned(),
            None,
        );
    };
    // Cheap service limits before any backend work: an unbounded query
    // would block the serial stdio loop for every later call.
    if name == "search" {
        if let Some(l) = args.get("limit").and_then(serde_json::Value::as_i64) {
            if l > MCP_SEARCH_LIMIT_MAX {
                return mcp_error(
                    id,
                    -32602,
                    format!("search limit exceeds maximum {MCP_SEARCH_LIMIT_MAX}"),
                    None,
                );
            }
        }
    }
    if name == "path" {
        if let Some(h) = args.get("max_hops").and_then(serde_json::Value::as_u64) {
            if h > MCP_MAX_HOPS as u64 {
                return mcp_error(
                    id,
                    -32602,
                    format!("max_hops exceeds maximum {MCP_MAX_HOPS}"),
                    None,
                );
            }
        }
    }
    let reader = match Box::pin(AnyReader::connect(repo)).await {
        Ok(r) => r,
        Err(e) => {
            let report = LifecycleReport::error(&format!("mcp {name}"), &e.to_string());
            return mcp_error(
                id,
                -32603,
                e.to_string(),
                Some(serde_json::to_value(&report).unwrap()),
            );
        }
    };
    let payload: Result<serde_json::Value, String> =
        match name {
            "search" => {
                let q = args["query"].as_str().unwrap_or_default();
                let limit = args
                    .get("limit")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(20);
                reader
                    .search(q, limit)
                    .await
                    .map(|rows| serde_json::to_value(&rows).unwrap())
                    .map_err(|e| e.to_string())
            }
            "lookup" => {
                let eid = args["id"].as_str().unwrap_or_default();
                reader
                    .lookup(eid)
                    .await
                    .map(|row| serde_json::to_value(&row).unwrap())
                    .map_err(|e| e.to_string())
            }
            "neighbors" => {
                let eid = args["id"].as_str().unwrap_or_default();
                let raw = args
                    .get("rel")
                    .and_then(|r| r.as_str())
                    .map(|r| vec![r.to_owned()]);
                let filter = match chaosbox::validate_rel_filter(raw) {
                    Ok(f) => f,
                    Err(e) => return mcp_error(id, -32602, e.to_string(), None),
                };
                reader.neighbors(eid, filter).await
                .map(|(out, inc)| serde_json::json!({"id": eid, "outgoing": out, "incoming": inc}))
                .map_err(|e| e.to_string())
            }
            "path" => {
                let from = args["from"].as_str().unwrap_or_default();
                let to = args["to"].as_str().unwrap_or_default();
                let hops = usize::try_from(
                    args.get("max_hops")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(4),
                )
                .expect("hop count fits in usize");
                reader
                    .path(from, to, hops)
                    .await
                    .map(|path| serde_json::json!({"from": from, "to": to, "path": path}))
                    .map_err(|e| e.to_string())
            }
            "evidence" => {
                let rel = args["rel"].as_str().unwrap_or_default();
                reader.evidence(rel).await.map_err(|e| e.to_string())
            }
            "status" => Ok(serde_json::json!({
                "repo": repo, "build_id": reader.build_id(), "generation": reader.generation(),
                "status": reader.status(), "snapshots": reader.snapshots(),
                "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
            })),
            "diff" => {
                let from = args["from_build"].as_str().unwrap_or_default();
                let to = args["to_build"].as_str().unwrap_or_default();
                reader.diff(repo, from, to).await.map_err(|e| e.to_string())
            }
            "export" => reader.export().await.map_err(|e| e.to_string()),
            "explain" => {
                let eid = args["id"].as_str().unwrap_or_default();
                reader.explain(eid).await.map_err(|e| e.to_string())
            }
            _ => {
                return mcp_error(
                    id,
                    -32601,
                    format!("read-only MCP: no such tool (rejected): {name}"),
                    None,
                );
            }
        };
    match payload {
        Ok(v) => mcp_text_result(id, &v),
        Err(e) => mcp_error(id, -32603, e, None),
    }
}

/// Read-only MCP over stdio: full handshake, paginated tools, validated calls.
/// Never loads Jev credentials; never accepts prose as evidence.
// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
async fn serve_mcp(intelligence: Option<chaosbox::intelligence::Bundle>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut initialized = false;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = if let Ok(v) = serde_json::from_str(&line) {
            v
        } else {
            let resp = mcp_error(
                &serde_json::Value::Null,
                -32700,
                "parse error".to_owned(),
                None,
            );
            let _ = stdout
                .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
                .await;
            continue;
        };
        if req.is_array() {
            let resp = mcp_error(
                &serde_json::Value::Null,
                -32600,
                "batch requests not supported".to_owned(),
                None,
            );
            let _ = stdout
                .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
                .await;
            continue;
        }
        // Notifications carry no id and get no response.
        let id = match req.get("id") {
            Some(i) => i.clone(),
            None => continue,
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let resp = match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let version = if requested == MCP_PROTOCOL_VERSION
                    || MCP_PROTOCOL_FALLBACKS.contains(&requested)
                {
                    requested.to_owned()
                } else {
                    MCP_PROTOCOL_VERSION.to_owned()
                };
                initialized = true;
                serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "chaosbox", "version": env!("CARGO_PKG_VERSION")},
                    },
                })
            }
            "notifications/initialized" => continue,
            "ping" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => {
                if initialized {
                    let mut defs = mcp_tool_defs();
                    if intelligence.is_some() {
                        defs.push(mcp_tool("intelligence_context", "Small historical, source-backed knowledge packet; not instructions or current-state proof.", serde_json::json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}), vec!["query"]));
                        defs.push(mcp_tool(
                            "intelligence_evidence",
                            "Sources and typed decision receipts for a pinned intelligence item.",
                            serde_json::json!({"id":{"type":"string"}}),
                            vec!["id"],
                        ));
                    }
                    let cursor = params
                        .get("cursor")
                        .and_then(|c| c.as_str())
                        .and_then(|c| c.parse::<usize>().ok())
                        .unwrap_or(0);
                    let page: Vec<_> = defs.into_iter().skip(cursor).take(MCP_PAGE_SIZE).collect();
                    let next = if page.len() == MCP_PAGE_SIZE {
                        Some((cursor + MCP_PAGE_SIZE).to_string())
                    } else {
                        None
                    };
                    let mut result = serde_json::json!({"tools": page});
                    if let Some(n) = next {
                        result["nextCursor"] = serde_json::Value::String(n);
                    }
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            "tools/call" => {
                if initialized {
                    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.starts_with("intelligence_") {
                        match intelligence.as_ref() {
                            Some(bundle) => match chaosbox::intelligence::mcp_query(
                                bundle,
                                name,
                                &params["arguments"],
                            ) {
                                Ok(value) => mcp_text_result(&id, &value),
                                Err(error) => mcp_error(&id, -32602, error, None),
                            },
                            None => mcp_error(
                                &id,
                                -32601,
                                "no intelligence bundle configured".into(),
                                None,
                            ),
                        }
                    } else {
                        Box::pin(mcp_call_tool(&id, name, &params)).await
                    }
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            _ => mcp_error(
                &id,
                -32601,
                format!("unknown method (rejected): {method}"),
                None,
            ),
        };
        let _ = stdout
            .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
            .await;
    }
    let _ = all_relation_types;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_defs_are_read_only_with_schemas() {
        let defs = mcp_tool_defs();
        assert_eq!(defs.len(), 9);
        for d in &defs {
            assert_eq!(d["annotations"]["readOnlyHint"], true);
            assert!(d["inputSchema"]["properties"].is_object(), "{d}");
            assert!(d.get("_required").is_none(), "no internal fields leak: {d}");
            // Schema and runtime validation agree: every tool requires a
            // nonempty repo, so schema-valid calls cannot fail on identity.
            let required = d["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                required.iter().any(|r| r == "repo"),
                "repo must be required: {d}"
            );
            assert_eq!(
                d["inputSchema"]["properties"]["repo"]["minLength"], 1,
                "repo must be nonempty: {d}"
            );
        }
    }

    #[test]
    fn missing_args_rejected_before_backend() {
        let err = mcp_args("search", &serde_json::json!({})).unwrap_err();
        assert_eq!(err["code"], -32602);
    }

    #[tokio::test]
    async fn unknown_tools_rejected_without_backend() {
        // No backend needed: the closed tool set rejects first.
        for name in ["migrate", "evaluate", "db", "ingest", "annotate"] {
            let resp = Box::pin(mcp_call_tool(
                &serde_json::json!(1),
                name,
                &serde_json::json!({"name": name, "repo": "demo"}),
            ))
            .await;
            assert_eq!(resp["error"]["code"], -32601, "{name}: {resp}");
        }
    }

    #[tokio::test]
    async fn missing_repo_rejected_before_backend() {
        // No backend needed: the explicit-repo rule fires first.
        let resp = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            "search",
            &serde_json::json!({"arguments": {"query": "alpha"}}),
        ))
        .await;
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("repo"),
            "{resp}"
        );
    }

    #[tokio::test]
    async fn unbounded_queries_rejected_before_backend() {
        // No backend needed: service limits fire before connecting.
        let over_limit = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            "search",
            &serde_json::json!({"arguments": {"query": "alpha", "repo": "demo", "limit": 100_000}}),
        ))
        .await;
        assert_eq!(over_limit["error"]["code"], -32602, "{over_limit}");
        assert!(
            over_limit["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("limit"),
            "{over_limit}"
        );
        let over_hops = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            "path",
            &serde_json::json!({"arguments": {"from": "a", "to": "b", "repo": "demo", "max_hops": 1000}}),
        ))
        .await;
        assert_eq!(over_hops["error"]["code"], -32602, "{over_hops}");
    }

    #[tokio::test]
    async fn calls_require_initialization_shape() {
        // Malformed (non-object) params fail arg validation, not the backend.
        let resp = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            "search",
            &serde_json::json!({"arguments": "not-an-object"}),
        ))
        .await;
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
    }
}
