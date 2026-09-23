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

mod backend_cmd;
mod mcp_server;
mod pipeline_cmd;
mod query_cmd;

use backend_cmd::{
    AnyReader, Backend, backend, consumer_err, db_check_typedb, run_migrate_typedb,
    typedb_config_from_env, typedb_publication_chain,
};
use mcp_server::serve_mcp;
use pipeline_cmd::run_pipeline_with;
use query_cmd::run_query;

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
    /// Inspect and verify a session-migration campaign.
    Sessions {
        #[command(subcommand)]
        command: chaosbox::sessions::cli::Command,
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
        Command::Sessions { command } => match chaosbox::sessions::cli::run(command) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("sessions: {error}");
                std::process::exit(1);
            }
        },
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

#[cfg(test)]
mod mcp_tests;
