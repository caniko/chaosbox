//! Typed operator interface. Every materializing operation carries a native reason.
use std::path::PathBuf;
use clap::Subcommand;
use serde_json::{json, Value};
use super::{AddMode, Invocation, Ledger, Settings, VERSION};

/// Closed operation vocabulary; no shell passthrough or arbitrary Nix argv.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Report the runtime contract before enabling a mutation adapter.
    Capabilities,
    /// Add a local file/directory, equivalent to nix-store --add / -A.
    Add {
        /// Why this object is needed; mandatory and nonblank in the execution layer.
        #[arg(long)]
        reason: String,
        /// Explicit repository/location association.
        #[arg(long)]
        repo: String,
        /// Stable id for exact retry. Omission generates a new invocation.
        #[arg(long)]
        id: Option<String>,
        /// Native `OpenCode` session.
        #[arg(long)]
        session: Option<String>,
        /// Native assistant message.
        #[arg(long)]
        message: Option<String>,
        /// Native tool-call identity.
        #[arg(long)]
        tool_call: Option<String>,
        /// Addressing method, never arbitrary arguments.
        #[arg(long, value_enum, default_value_t=AddMode::Nar)]
        mode: AddMode,
        /// File/directory, passed to Nix after an argument terminator.
        #[arg(last = true)]
        path: PathBuf,
    },
    /// Offline operational intelligence with source-backed evidence handles.
    Context {
        /// Repository filter.
        #[arg(long)]
        repo: String,
        /// Literal lexical query.
        query: String,
        /// Maximum records (1..20).
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Read one immutable invocation and its actual settlement.
    Evidence {
        /// Required repository boundary.
        #[arg(long)]
        repo: String,
        /// Exact operation identity.
        id: String,
    },
    /// Read-only cleanup view; does not authorize removal or perform GC.
    Query {
        /// Exact returned store path, if selecting its consumers.
        #[arg(long)]
        path: Option<String>,
        /// Bounded pagination cursor.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Maximum records (1..100).
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

/// Execute against operator-pinned settings and return bounded JSON.
pub async fn run(command: Command) -> Result<Value, String> {
    if matches!(command, Command::Capabilities) {
        return Ok(
            json!({"version":VERSION,"operations":["add"],"retention":["unrooted"],"queries":["context","evidence","query"],"mutation_authorization":"caller-owned"}),
        );
    }
    let settings = Settings::from_env()?;
    match command {
        Command::Capabilities => Ok(
            json!({"version":VERSION,"operations":["add"],"retention":["unrooted"],"queries":["context","evidence","query"],"mutation_authorization":"caller-owned"}),
        ),
        Command::Add {
            reason,
            repo,
            id,
            session,
            message,
            tool_call,
            mode,
            path,
        } => {
            let cwd = std::env::current_dir().map_err(super::err)?;
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            let id = if let Some(id) = id {
                id
            } else {
                let mut random = [0_u8; 16];
                ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random)
                    .map_err(super::err)?;
                format!("nix:{}", hex::encode(random))
            };
            super::add(
                &settings,
                Invocation {
                    id,
                    repo,
                    reason,
                    cwd,
                    session,
                    message,
                    tool_call,
                    path,
                    mode,
                },
            )
            .await
        }
        Command::Context { repo, query, limit } => {
            Ledger::read(&settings)?.context(&repo, &query, limit)
        }
        Command::Evidence { repo, id } => Ledger::read(&settings)?.evidence(&repo, &id),
        Command::Query {
            path,
            offset,
            limit,
        } => Ledger::read(&settings)?.query(path.as_deref(), offset, limit),
    }
}
