//! `chaosbox` CLI + read-only MCP server.
//!
//! Consumers (CLI queries, MCP tools) share one query implementation in the
//! library. MCP is read-only: no mutation, ingestion, annotations, arbitrary
//! EdgeQL/SQL, migrations, or model configuration tools. The read-only server
//! never loads Jev credentials. Indexing/administration are operator commands.

use std::{
    collections::BTreeMap,
    io::{BufRead, Write as _},
    path::PathBuf,
};

use chaosbox::{FixtureResponder, LifecycleReport, Materialization, Pipeline, db_check_report, explain_entity, export_json, search};
use chaosbox_extract::Snapshot;
use chaosbox_gel::MemoryStore;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "chaosbox", version, about = "Chaosbox deterministic code-graph pipeline (Gel-backed)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Snapshot a fixture repository.
    Snapshot { path: PathBuf, #[arg(long, default_value = "demo")] repo: String },
    /// Extract deterministic facts + candidates.
    Extract { path: PathBuf, #[arg(long, default_value = "demo")] repo: String },
    /// Run the full pipeline against a local protocol fixture (no creds).
    Run {
        path: PathBuf,
        #[arg(long, default_value = "demo")] repo: String,
        #[arg(long, default_value_t = 200)] max_candidates: usize,
    },
    /// Query helpers (read-only; share lib implementation with MCP).
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
    Search { query: String, #[arg(long, default_value_t = 20)] limit: usize },
    Lookup { id: String },
    Neighbors { id: String, #[arg(long)] rel: Option<String> },
    Path { from: String, to: String, #[arg(long, default_value_t = 4)] max_hops: usize },
    Export {},
}

#[derive(Debug, Subcommand)]
enum DbCmd {
    /// Read-only readiness check. Exit 0 only when ready.
    Check {
        #[arg(long)] json: bool,
        #[arg(long, default_value = "demo")] repo: String,
    },
    /// Apply committed migrations idempotently via pinned Gel tooling.
    Migrate {
        #[arg(long)] json: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Snapshot { path, repo } => {
            match Snapshot::capture(&repo, &path) {
                Ok(s) => println!(r#"{{"snapshot":"{}","files":{}}}"#, s.id, s.files.len()),
                Err(e) => {
                    eprintln!("snapshot failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Command::Extract { path, repo } => {
            match Pipeline::snapshot_extract(&repo, &path, 200) {
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
            }
        }
        Command::Run { path, repo, max_candidates } => {
            let code = run_pipeline(&path, &repo, max_candidates).await;
            std::process::exit(code);
        }
        Command::Query { q } => {
            eprintln!("query needs a published build; use `chaosbox run` output or MCP with a build file");
            let _ = q;
            std::process::exit(2);
        }
        Command::Mcp => serve_mcp(),
        Command::Db { op } => match op {
            DbCmd::Check { json: _, repo } => {
                // Read-only: never init/migrate/repair. No active build in a
                // fresh process => pending (nonzero), diagnostics to stderr.
                let store = MemoryStore::new();
                let report = db_check_report(&store, &repo);
                println!("{}", serde_json::to_string(&report).unwrap());
                if report.status == "ready" {
                    std::process::exit(0);
                } else {
                    eprintln!("not ready: {}", report.status);
                    std::process::exit(1);
                }
            }
            DbCmd::Migrate { json: _ } => {
                // Idempotent committed migrations via pinned Gel CLI when a
                // credentials file is present; refuse divergent history.
                match run_migrate().await {
                    Ok(report) => {
                        println!("{}", serde_json::to_string(&report).unwrap());
                        std::process::exit(if report.status == "ready" { 0 } else { 1 });
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

async fn run_pipeline(path: &PathBuf, repo: &str, max_candidates: usize) -> i32 {
    let (snap, ext, cands) = match Pipeline::snapshot_extract(repo, path, max_candidates) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("extract: {e}");
            return 1;
        }
    };
    let entities: BTreeMap<_, _> = ext.entities.iter().map(|e| (e.id.clone(), e.clone())).collect();
    let mut responder = FixtureResponder::new(true);
    let decided = match Pipeline::decide(&cands, &entities, &mut responder, chaosbox_jev::JEV_MODEL_PINNED).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("decide: {e}");
            return 1;
        }
    };
    let mut pipe = Pipeline::new();
    let mat = Materialization::default();
    match pipe.build_and_publish(repo, &snap, &ext, &decided, &mat, None) {
        Ok(build) => {
            let v = export_json(&build);
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
    let creds = std::env::var("CHAOSBOX_GEL_CREDENTIALS_FILE").map_err(|_| "CHAOSBOX_GEL_CREDENTIALS_FILE unset".to_owned())?;
    // Never log secret values; only reference the file.
    let out = tokio::process::Command::new("gel")
        .args(["migration", "apply", "--non-interactive"])
        .env("GEL_CREDENTIALS_FILE", &creds)
        .output()
        .await
        .map_err(|e| format!("gel CLI: {e}"))?;
    if out.status.success() {
        Ok(LifecycleReport::check_ready(serde_json::json!({"applied": true})))
    } else {
        Err(format!("gel migration apply failed: {}", String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()))
    }
}

/// Minimal read-only MCP (JSON-RPC over stdio).
///
/// Tools: search, lookup, neighbors, path, evidence, status, diff, export,
/// explain. Any write/mutation/EdgeQL/migration/model tool is rejected.
/// No Jev credential is loaded here by construction.
fn serve_mcp() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // Note: serving without a loaded build answers status only; a full
    // deployment injects the pinned active build. Never accepts prose as evidence.
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(stdout, r#"{{"error":"parse: {e}"}}"#);
                continue;
            }
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let resp = match method {
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0", "id": id, "result": {"tools": [
                    {"name": "search"}, {"name": "lookup"}, {"name": "neighbors"},
                    {"name": "path"}, {"name": "evidence"}, {"name": "status"},
                    {"name": "diff"}, {"name": "export"}, {"name": "explain"},
                ]}}),
            "tools/call" => {
                let name = req.pointer("/params/name").and_then(|n| n.as_str()).unwrap_or("");
                match name {
                    "search" | "lookup" | "neighbors" | "path" | "evidence" | "status" | "diff" | "export" | "explain" => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"status": "ok", "note": "read-only; no build loaded in this demo invocation"}}),
                    _ => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("read-only MCP: no such tool (rejected): {name}")}}),
                }
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "unknown method"}}),
        };
        let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
    }
    let _ = (search, explain_entity);
}
