//! Operator writes and bounded read-only scratch intelligence for cleanup tools.
use std::{io::Read, path::PathBuf};
use clap::{Args, Subcommand};
use serde_json::Value;
use super::{Ledger, QueryPath, Request, MAX_BYTES, Event, assessment::Budget};

/// Fixed operator identity, shared by every command.
#[derive(Debug, Args)]
pub struct Settings {
    /// Durable private custody directory outside scratch.
    #[arg(long, env = "CHAOSBOX_SCRATCH_WORK")]
    pub work: PathBuf,
    /// Operator-owned scope.
    #[arg(long, env = "CHAOSBOX_SCRATCH_SCOPE")]
    pub scope: String,
    /// Host namespace (the adapter defaults to the local hostname).
    #[arg(long, env = "CHAOSBOX_SCRATCH_HOST")]
    pub host: String,
    /// Tracked scratch root.
    #[arg(
        long,
        env = "CHAOSBOX_SCRATCH_ROOT",
        default_value = "/data/scratch/tmp/opencode"
    )]
    pub root: PathBuf,
}

/// Scratch ledger administration and cleanup intelligence.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Assess changed/stale workspace evidence with the pinned Jev model.
    Assess {
        /// Exact allocation paths; include ancestors/descendants. Empty selects stale work.
        paths: Vec<PathBuf>,
        /// Send retained, bounded native text and linked evidence to Jev.
        #[arg(long, required = true)]
        privacy_reviewed: bool,
        /// Persistent request ceiling, including previous runs and failures.
        #[arg(long, default_value_t = 1000)]
        max_requests: u32,
        /// Persistent conservative input-token ceiling.
        #[arg(long, default_value_t = 10_000_000)]
        max_input_tokens: u64,
        /// Explicitly retry failed/interrupted assessments.
        #[arg(long)]
        retry: bool,
    },
    /// Link artifact, patch, test, commit or preservation evidence as an assertion.
    Link {
        /// Existing allocation.
        path: PathBuf,
        /// Evidence category.
        #[arg(long, value_parser = ["patch","test","commit","artifact","preservation"])]
        category: String,
        /// Exact reference or destination; never treated as verified by itself.
        #[arg(long)]
        reference: String,
        /// Why this evidence applies here.
        #[arg(long)]
        description: String,
        /// Native source session, when linked by the `OpenCode` adapter.
        #[arg(long)]
        session: Option<String>,
        /// Native source message.
        #[arg(long)]
        message: Option<String>,
    },
    /// Recover a complete replayable Jev receipt from private custody.
    Receipt {
        /// Assessment id from a query packet.
        id: String,
    },
    /// Read durable worker spending, failures and interrupted attempts without credentials.
    AssessmentStatus,
    /// Commit a versioned event batch from stdin.
    Record,
    /// List allocations, with source-backed purpose and cleanup holds.
    List {
        /// Maximum entries scanned; incomplete coverage is explicit.
        #[arg(long, default_value_t = 1000)]
        limit: usize,
    },
    /// Explain a path, including protected ancestors and descendants.
    Explain {
        /// Exact absolute path.
        path: PathBuf,
        /// Maximum entries scanned.
        #[arg(long, default_value_t = 10_000)]
        limit: usize,
    },
    /// Read a bounded batch of exact absolute paths from a JSON array on stdin.
    Query {
        /// Maximum entries scanned.
        #[arg(long, default_value_t = 10_000)]
        limit: usize,
    },
    /// Recover an exact native purpose-text projection by content hash.
    Evidence {
        /// Source hash returned by list/explain/query.
        hash: String,
    },
    /// Explicitly reconcile an orphaned process after checking it has stopped.
    Resolve {
        /// Invocation id returned by explain (`tool:` or `shell:`).
        invocation: String,
        /// Why the operator knows the process is no longer running.
        #[arg(long)]
        reason: String,
    },
    /// Record purpose or remaining finalization work.
    Annotate {
        /// Existing allocation.
        path: PathBuf,
        /// Purpose or outstanding work.
        #[arg(long)]
        reason: String,
        /// Explicit disposition.
        #[arg(long, default_value = "needs-finalization", value_parser = ["open","needs-finalization","unknown"])]
        disposition: String,
    },
    /// Explicitly release an allocation; command success never does this.
    Release {
        /// Existing allocation.
        path: PathBuf,
        /// Why its outputs are now disposable or preserved.
        #[arg(long)]
        reason: String,
        /// Optional exact preservation receipt (destination/hash/commit).
        #[arg(long)]
        receipt: Option<String>,
    },
}

fn stdin_value<T: serde::de::DeserializeOwned>() -> Result<T, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("scratch input exceeds 4 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// CLI dispatch. Query commands never load model/database credentials or write.
pub async fn run(settings: &Settings, command: Command) -> Result<Value, String> {
    if settings.work.starts_with(&settings.root) || settings.root.starts_with(&settings.work) {
        return Err("scratch ledger state and tracked root must be separate".into());
    }
    if let Command::Assess {
        paths,
        privacy_reviewed,
        max_requests,
        max_input_tokens,
        retry,
    } = command
    {
        if !privacy_reviewed {
            return Err("scratch Jev assessment requires reviewed evidence".into());
        }
        let client = chaosbox_jev::JevClient::new(chaosbox_jev::JevPolicy {
            max_requests,
            max_input_tokens,
            max_retries: 0,
            ..Default::default()
        })
        .map_err(|_| "scratch Jev client unavailable".to_string())?;
        let mut responder = crate::LiveResponder::new(client);
        return Ledger::open(&settings.work,&settings.scope)?.assess(&mut responder,&settings.host,&settings.root,&paths,Budget { requests:max_requests,input_tokens:max_input_tokens,retry }).await
                .map_err(|_| "scratch assessment pending, failed or budget-exhausted; use explicit retry after reviewing the durable attempts".into());
    }
    run_local(settings, command)
}

fn run_local(settings: &Settings, command: Command) -> Result<Value, String> {
    match command {
        Command::Assess { .. } => Err("assessment requires asynchronous dispatch".into()),
        Command::AssessmentStatus => {
            Ledger::read(&settings.work, &settings.scope)?.assessment_status()
        }
        Command::Link {
            path,
            category,
            reference,
            description,
            session,
            message,
        } => link(
            settings,
            Event::Link {
                id: format!(
                    "link:{}:{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos()
                ),
                identity: super::identity(&path, &settings.root)?,
                path,
                category,
                reference,
                description,
                session,
                message,
            },
        ),
        Command::Receipt { id } => receipt(settings, &id),
        Command::Record => {
            let request: Request = stdin_value()?;
            if request.scope != settings.scope
                || request.host != settings.host
                || request.root != settings.root
            {
                return Err("scratch producer identity differs from operator settings".into());
            }
            Ledger::open(&settings.work, &settings.scope)?.record(&request)
        }
        Command::Annotate {
            path,
            reason,
            disposition,
        } => Ledger::open(&settings.work, &settings.scope)?.annotate(
            &settings.host,
            &settings.root,
            &path,
            &disposition,
            &reason,
            None,
        ),
        Command::Release {
            path,
            reason,
            receipt,
        } => Ledger::open(&settings.work, &settings.scope)?.annotate(
            &settings.host,
            &settings.root,
            &path,
            "released",
            &reason,
            receipt.as_deref(),
        ),
        Command::List { limit } => Ledger::read(&settings.work, &settings.scope)?.query(
            &settings.host,
            &settings.root,
            &[],
            limit,
        ),
        Command::Explain { path, limit } => Ledger::read(&settings.work, &settings.scope)?.query(
            &settings.host,
            &settings.root,
            &[path],
            limit,
        ),
        Command::Query { limit } => query(settings, limit),
        Command::Evidence { hash } => {
            Ledger::read(&settings.work, &settings.scope)?.evidence(&hash)
        }
        Command::Resolve { invocation, reason } => Ledger::open(&settings.work, &settings.scope)?
            .resolve(&settings.host, &settings.root, &invocation, &reason),
    }
}

fn link(settings: &Settings, event: Event) -> Result<Value, String> {
    Ledger::open(&settings.work, &settings.scope)?.record(&Request {
        version: 1,
        scope: settings.scope.clone(),
        host: settings.host.clone(),
        root: settings.root.clone(),
        events: vec![event],
    })
}

fn receipt(settings: &Settings, id: &str) -> Result<Value, String> {
    let ledger = Ledger::read(&settings.work, &settings.scope)?;
    let raw: String = ledger
        .db
        .query_row(
            "SELECT body FROM scratch_assessments WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let mut body = value.clone();
    body.as_object_mut()
        .ok_or("invalid assessment receipt")?
        .remove("id");
    if value["id"] != id
        || chaosbox_core::sha256_hex(&["scratch-assessment-v1", &body.to_string()]) != id
    {
        return Err("scratch assessment receipt digest mismatch".into());
    }
    Ok(value)
}

fn query(settings: &Settings, limit: usize) -> Result<Value, String> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum PathInput {
        Plain(PathBuf),
        Located(QueryPath),
    }
    let paths = stdin_value::<Vec<PathInput>>()?
        .into_iter()
        .map(|p| match p {
            PathInput::Plain(path) => QueryPath { path, at: None },
            PathInput::Located(p) => p,
        })
        .collect::<Vec<_>>();
    Ledger::read(&settings.work, &settings.scope)?.query_at(
        &settings.host,
        &settings.root,
        &paths,
        limit,
    )
}
