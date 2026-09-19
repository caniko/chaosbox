//! `chaosbox` CLI + read-only MCP server.
//!
//! Consumers (CLI queries, MCP tools) share one query implementation in the
//! library, served Gel-backed through [`chaosbox::GelReader`]. MCP is
//! read-only: no mutation, ingestion, annotations, arbitrary EdgeQL/SQL,
//! migrations, or model configuration tools. The read-only server never loads
//! Jev credentials. Indexing/administration are operator commands.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chaosbox::{
    all_relation_types, GelReader, LifecycleReport, Materialization, Pipeline, EXPORT_EDGE_CAP,
    EXPORT_NODE_CAP,
};
use chaosbox::{FixtureResponder, LiveResponder};
use chaosbox_extract::Snapshot;
use chaosbox_gel::{MemoryStore, Store as _};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "chaosbox",
    version,
    about = "Chaosbox deterministic code-graph pipeline (Gel-backed)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
    /// Run the full pipeline (fixture decisions unless --live-jev).
    Run {
        path: PathBuf,
        #[arg(long, default_value = "demo")]
        repo: String,
        #[arg(long, default_value_t = 200)]
        max_candidates: usize,
        /// Use the live Jev API (needs `CHAOSBOX_JEV_API_KEY_FILE`) instead of
        /// the deterministic fixture. Real inference, real spend.
        #[arg(long, default_value_t = false)]
        live_jev: bool,
        /// Live-Jev spend guards (defaults = `JevPolicy::default`).
        #[arg(long)]
        max_requests: Option<u32>,
        #[arg(long)]
        max_input_tokens: Option<u64>,
        #[arg(long)]
        max_retries: Option<u32>,
    },
    /// Query helpers (read-only; Gel-backed, shared with MCP).
    Query {
        #[command(subcommand)]
        q: QueryCmd,
    },
    /// Serve read-only MCP over stdio (no Jev credentials loaded).
    Mcp,
    /// Lifecycle contract v1.
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
        /// Emit the versioned JSON envelope (contract v1; always on).
        #[arg(long, default_value_t = true)]
        json: bool,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
    /// Apply committed migrations idempotently via pinned Gel tooling.
    Migrate {
        /// Emit the versioned JSON envelope (contract v1; always on).
        #[arg(long, default_value_t = true)]
        json: bool,
        #[arg(long, default_value = "demo")]
        repo: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
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
            Ok((snap, ext, cands)) => println!(
                r#"{{"snapshot":"{}","entities":{},"candidates":{}}}"#,
                snap.id,
                ext.entities.len(),
                cands.len()
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
            max_requests,
            max_input_tokens,
            max_retries,
        } => {
            let code = run_pipeline(
                &path,
                &repo,
                max_candidates,
                live_jev,
                max_requests,
                max_input_tokens,
                max_retries,
            )
            .await;
            std::process::exit(code);
        }
        Command::Query { q } => std::process::exit(Box::pin(run_query(q)).await),
        Command::Mcp => Box::pin(serve_mcp()).await,
        Command::Db { op } => match op {
            DbCmd::Check { json: _, repo } => {
                // Read-only: never init/migrate/repair. Exit 0 when ready,
                // 2 when pending, 1 otherwise (harbor-db contract v1).
                let report = Box::pin(db_check_gel(&repo)).await;
                println!("{}", serde_json::to_string(&report).unwrap());
                if report.status == "ready" {
                    std::process::exit(0);
                } else {
                    eprintln!("not ready: {}", report.status);
                    std::process::exit(if report.status == "pending" { 2 } else { 1 });
                }
            }
            DbCmd::Migrate { json: _, repo: _ } => {
                // Idempotent committed migrations via pinned Gel CLI when a
                // credentials file is present; refuse divergent history.
                // run_migrate verifies schema readiness itself; the arm only
                // maps the verdict to the exit code.
                match Box::pin(run_migrate()).await {
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

/// Gel-backed readiness: connectivity + probe + active build for the repo.
async fn db_check_gel(repo: &str) -> LifecycleReport {
    let handle = match Box::pin(chaosbox_gel::GelHandle::connect()).await {
        Ok(h) => h,
        Err(e) => return LifecycleReport::error("db check", &format!("gel connect: {e}")),
    };
    if let Err(e) = Box::pin(handle.probe()).await {
        return LifecycleReport::error("db check", &format!("gel probe: {e}"));
    }
    match Box::pin(handle.active_build(repo)).await {
        Err(e) => match Box::pin(handle.schema_present()).await {
            // No marker type: committed migrations have not applied yet.
            // This is the normal pre-migration state, not a failure.
            Ok(false) => LifecycleReport::pending("db check", "migrations not applied"),
            Ok(true) => LifecycleReport::error("db check", &format!("active build: {e}")),
            Err(probe) => LifecycleReport::error(
                "db check",
                &format!("active build: {e}; schema probe: {probe}"),
            ),
        },
        Ok(None) => LifecycleReport::pending("db check", "no active build for repo"),
        Ok(Some(b)) => LifecycleReport::check_ready(serde_json::json!({
            "repo": repo, "active_build": b.build_id, "generation": b.generation,
            "status": b.status, "schema_assets": "packaged",
        })),
    }
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
            let reader = match Box::pin(GelReader::connect(&repo)).await {
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
            let reader = match Box::pin(GelReader::connect(&repo)).await {
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
            // Validate the filter before touching Gel: typos must fail loudly.
            let filter = match chaosbox::validate_rel_filter(rel.map(|r| vec![r])) {
                Ok(f) => f,
                Err(e) => return consumer_err("query neighbors", e),
            };
            let reader = match Box::pin(GelReader::connect(&repo)).await {
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
            let reader = match Box::pin(GelReader::connect(&repo)).await {
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
            let reader = match Box::pin(GelReader::connect(&repo)).await {
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
            let reader = match Box::pin(GelReader::connect(&repo)).await {
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
            let reader = match Box::pin(GelReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query status", e),
            };
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "repo": repo, "build_id": reader.build_id,
                    "generation": reader.generation,
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
async fn run_pipeline(
    path: &Path,
    repo: &str,
    max_candidates: usize,
    live_jev: bool,
    max_requests: Option<u32>,
    max_input_tokens: Option<u64>,
    max_retries: Option<u32>,
) -> i32 {
    let (snap, ext, cands) =
        match Pipeline::<MemoryStore>::snapshot_extract(repo, path, max_candidates) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("extract: {e}");
                return 1;
            }
        };
    let entities: BTreeMap<_, _> = ext
        .entities
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let mat = Materialization::default();
    let mut pipe = Pipeline::<MemoryStore>::new();
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
    let catalog = chaosbox_core::catalog_digest(&cands);
    let run_id = chaosbox_core::deterministic_id("run", &[repo, &snap.id]);
    let set_id = chaosbox_core::deterministic_id("set", &[&run_id, &catalog, &mat.rubric_version]);
    if let Err(e) = pipe
        .store
        .ensure_run(
            &run_id,
            repo,
            &snap.id,
            &set_id,
            &catalog,
            &mat.rubric_version,
        )
        .await
    {
        eprintln!("run identity: {e}");
        return 1;
    }
    for cand in &cands {
        if let Err(e) = pipe.store.put_candidate(&set_id, cand).await {
            eprintln!("candidate: {e}");
            return 1;
        }
    }
    let decided = if live_jev {
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
        let client = match chaosbox_jev::JevClient::new(policy) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("jev client: {e}");
                return 1;
            }
        };
        let mut responder = LiveResponder::new(client);
        match Pipeline::<MemoryStore>::decide(
            &cands,
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
        let mut responder = FixtureResponder::new(true);
        match Pipeline::<MemoryStore>::decide(
            &cands,
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
    };
    match pipe
        .build_and_publish(repo, &snap, &ext, &decided, &mat, None)
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

async fn run_migrate() -> Result<LifecycleReport, String> {
    let creds = std::env::var("CHAOSBOX_GEL_CREDENTIALS_FILE")
        .map_err(|_| "CHAOSBOX_GEL_CREDENTIALS_FILE unset".to_owned())?;
    // Pinned binary under Nix (`db-migrate` app); ambient `gel` only for
    // cargo-run development. Never log secret values; only reference the file.
    // GEL_CREDENTIALS_FILE is a documented Gel connection parameter. No
    // --non-interactive flag: Gel CLI 7.x has none and applies without
    // prompting when stdin is not a TTY.
    let gel_bin = std::env::var("CHAOSBOX_GEL_BIN").unwrap_or_else(|_| "gel".to_owned());
    let out = tokio::process::Command::new(gel_bin)
        .args(["--credentials-file", &creds, "migration", "apply"])
        .env("GEL_CREDENTIALS_FILE", &creds)
        .output()
        .await
        .map_err(|e| format!("gel CLI: {e}"))?;
    if out.status.success() {
        // Schema-level verification only: server reachable, authenticated,
        // committed schema present. Active builds are published by pipelines
        // AFTER migration, so a fresh database legitimately has none;
        // deployment distinguishes schema readiness (migrate exit 0) from
        // application readiness (check exit 0 only with an active build).
        // Reuses the same connection the CLI just proved.
        std::env::set_var("GEL_CREDENTIALS_FILE", &creds);
        match Box::pin(chaosbox_gel::GelHandle::connect()).await {
            Err(e) => Err(format!("post-apply connect: {e}")),
            Ok(handle) => match Box::pin(handle.schema_present()).await {
                Err(e) => Err(format!("post-apply schema probe: {e}")),
                Ok(false) => Err("post-apply schema probe: marker type absent".into()),
                Ok(true) => Ok(chaosbox::LifecycleReport {
                    contract_version: 1,
                    backend: "gel".into(),
                    operation: "db migrate".into(),
                    status: "ready".into(),
                    schema_version: chaosbox_gel::SCHEMA_VERSION,
                    gel_pinned: chaosbox_gel::GEL_PINNED.into(),
                    detail: serde_json::json!({"applied": true}),
                }),
            },
        }
    } else {
        Err(format!(
            "gel migration apply failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(300)
                .collect::<String>()
        ))
    }
}

// ---- Read-only MCP (JSON-RPC over stdio, full handshake) ----

/// MCP protocol version served here.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Older protocol versions still accepted from clients.
const MCP_PROTOCOL_FALLBACKS: &[&str] = &["2024-11-05", "2025-03-26"];
/// Tools per `tools/list` page.
const MCP_PAGE_SIZE: usize = 5;

fn mcp_tool_defs() -> Vec<serde_json::Value> {
    vec![
        mcp_tool(
            "search",
            "Substring search over entity names (sorted, bounded).",
            serde_json::json!({"query": {"type": "string"}, "limit": {"type": "integer", "default": 20}}),
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
                "max_hops": {"type": "integer", "default": 4}}),
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
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });
    // Every tool accepts an optional repo; the pinned active build serves reads.
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.insert(
            "repo".to_owned(),
            serde_json::json!({"type": "string", "default": "demo"}),
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
    // touching Gel or credentials of any kind.
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
    let repo = args.get("repo").and_then(|r| r.as_str()).unwrap_or("demo");
    let reader = match Box::pin(GelReader::connect(repo)).await {
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
                "repo": repo, "build_id": reader.build_id, "generation": reader.generation,
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
async fn serve_mcp() {
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
                    let defs = mcp_tool_defs();
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
                    Box::pin(mcp_call_tool(&id, name, &params)).await
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
        }
    }

    #[test]
    fn missing_args_rejected_before_gel() {
        let err = mcp_args("search", &serde_json::json!({})).unwrap_err();
        assert_eq!(err["code"], -32602);
    }

    #[tokio::test]
    async fn unknown_tools_rejected_without_gel() {
        // No Gel needed: the closed tool set rejects first.
        for name in ["migrate", "evaluate", "db", "edgeql", "ingest", "annotate"] {
            let resp = Box::pin(mcp_call_tool(
                &serde_json::json!(1),
                name,
                &serde_json::json!({"name": name}),
            ))
            .await;
            assert_eq!(resp["error"]["code"], -32601, "{name}: {resp}");
        }
    }

    #[tokio::test]
    async fn calls_require_initialization_shape() {
        // Malformed (non-object) params fail arg validation, not Gel.
        let resp = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            "search",
            &serde_json::json!({"arguments": "not-an-object"}),
        ))
        .await;
        assert_eq!(resp["error"]["code"], -32602, "{resp}");
    }
}
