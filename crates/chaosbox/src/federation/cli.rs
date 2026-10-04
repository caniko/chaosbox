//! Explicit read-only federation CLI; no policy writes, enrollment or inference.
use std::path::Path;
use clap::Subcommand;
use serde_json::Value;
use super::{ClientConfig, ErrorCode, Handle, transport};

/// Operator-pinned query commands, shared with MCP and the `OpenCode` adapter.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Query local first, then all selected project peers within one deadline.
    Context {
        /// Explicit shared project identity from the provider mappings.
        #[arg(long)]
        repo: String,
        /// Lexical task terms.
        query: String,
        /// Maximum combined records.
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Maximum serialized combined record-array characters.
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Retrieve evidence from the original provider and exact snapshot.
    Evidence {
        /// Shared project identity; must match the handle.
        #[arg(long)]
        repo: String,
        /// JSON evidence handle returned by context, never a file path.
        #[arg(long)]
        handle: String,
        /// Maximum serialized evidence packet characters.
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Serve one local-only read as a fixed authenticated recipient (forced SSH command).
    Serve {
        /// Recipient bound by `authorized_keys`, never supplied by the wire request.
        #[arg(long)]
        caller: String,
    },
}

/// Execute a closed read operation, using an owner/route configuration from disk.
pub async fn run(path: &Path, command: Command) -> Result<Option<Value>, ErrorCode> {
    if let Command::Serve { caller } = command {
        transport::serve(path, &caller).await?;
        return Ok(None);
    }
    let reader = ClientConfig::load(path)?.reader()?;
    let value = match command {
        Command::Context {
            repo,
            query,
            limit,
            max_chars,
        } => serde_json::to_value(reader.context(&repo, &query, limit, max_chars).await?),
        Command::Evidence {
            repo,
            handle,
            max_chars,
        } => {
            let handle: Handle =
                serde_json::from_str(&handle).map_err(|_| ErrorCode::InvalidRequest)?;
            if handle.project != repo {
                return Err(ErrorCode::Denied);
            }
            serde_json::to_value(reader.evidence(&handle, max_chars).await?)
        }
        Command::Serve { .. } => return Err(ErrorCode::InvalidRequest),
    }
    .map_err(|_| ErrorCode::InvalidResponse)?;
    Ok(Some(value))
}
