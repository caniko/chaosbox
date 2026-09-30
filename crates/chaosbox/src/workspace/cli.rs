//! Private artifact capture and read-only bounded impact query.

use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use clap::Subcommand;
use super::{capture, Result, Spec, Workspace};

/// Explicit workspace pilot operations.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Capture a reviewed recipe into a new private immutable artifact.
    Capture {
        /// JSON recipe with selected members, quoted endpoints and bridges.
        spec: PathBuf,
        /// New file; never overwrite an existing artifact.
        #[arg(long)]
        output: PathBuf,
    },
    /// Query bounded impact after checking member freshness and visibility.
    Impact {
        /// Immutable artifact.
        artifact: PathBuf,
        /// Endpoint label whose contract is changing.
        changed: String,
        /// Must exactly match the artifact visibility scope.
        #[arg(long)]
        scope: String,
        /// Maximum directed dependency hops (1..8).
        #[arg(long, default_value_t = 4)]
        max_hops: usize,
        /// Maximum impacted endpoints including the starting one (1..100).
        #[arg(long, default_value_t = 20)]
        max_nodes: usize,
    },
}

/// Execute one pilot operation. This surface performs no inference or deployment.
pub async fn run(command: Command) -> Result<serde_json::Value> {
    match command {
        Command::Capture { spec, output } => {
            let recipe: Spec = read_json(&spec)?;
            let artifact = capture(recipe).await?;
            let bytes = serde_json::to_vec(&artifact)?;
            if bytes.len() > 32 * 1024 * 1024 {
                return Err("workspace artifact exceeds 32 MiB".into());
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(output)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(
                serde_json::json!({"workspace":artifact.id,"scope":artifact.scope,"members":artifact.members.iter().map(|(repo,m)| (repo,&m.build.id)).collect::<std::collections::BTreeMap<_,_>>()}),
            )
        }
        Command::Impact {
            artifact,
            changed,
            scope,
            max_hops,
            max_nodes,
        } => {
            let artifact = load_workspace(&artifact)?;
            artifact.impact(&scope, &changed, max_hops, max_nodes)
        }
    }
}

/// Load a bounded, fingerprint-validated artifact for read-only CLI/MCP use.
pub fn load_workspace(path: &Path) -> Result<Workspace> {
    let workspace: Workspace = read_json(path)?;
    workspace.validate(&workspace.scope)?;
    Ok(workspace)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err("artifact must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("artifact exceeds 32 MiB".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
