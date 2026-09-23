//! Execute the pinned cutover tools.
//!
//! The restartable installer and the rollback already exist as rehearsed
//! scripts with 51 passing rehearsal cases behind them. Reimplementing either
//! in Rust would produce two authorities for one state file and throw that
//! evidence away, so `install` and `rollback` resolve the pinned script out
//! of the campaign's `tools.json`, verify its digest immediately before use,
//! and exec it — forwarding arguments and exit codes unchanged. A run whose
//! recorded tool digest does not match refuses to start, and scratch paths
//! are never consulted.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Resolve a pinned tool out of `<root>/tools.json`, verifying its digest
/// against the file on disk before anyone executes it.
///
/// # Errors
///
/// Returns a message when the pin record is missing or unreadable, when the
/// tool is not pinned, or when the file on disk does not hash to the pinned
/// digest.
pub fn resolve_tool(root: &Path, name: &str) -> Result<PathBuf, String> {
    let pins = root.join("tools.json");
    let text = fs::read_to_string(&pins)
        .map_err(|error| format!("cannot read {}: {error}", pins.display()))?;
    let pinned: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot parse {}: {error}", pins.display()))?;
    let entry = pinned
        .get("tools")
        .and_then(|tools| tools.get(name))
        .ok_or_else(|| format!("{name} is not pinned in {}", pins.display()))?;
    let path = entry
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} has no path in {}", pins.display()))?;
    let expected = entry
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} has no digest in {}", pins.display()))?;
    let actual =
        file_sha256(Path::new(path)).map_err(|error| format!("cannot hash {path}: {error}"))?;
    if actual != expected {
        return Err(format!(
            "{name} at {path} digests as {actual} instead of the pinned {expected}: refusing to run"
        ));
    }
    Ok(PathBuf::from(path))
}

/// Campaign root for tool resolution: the flag, or the environment.
///
/// # Errors
///
/// Returns a message when neither names a root.
pub fn tool_root(root: Option<PathBuf>) -> Result<PathBuf, String> {
    root.or_else(|| {
        std::env::var("CHAOSBOX_SESSION_CAMPAIGN")
            .map(PathBuf::from)
            .ok()
    })
    .ok_or_else(|| "no campaign root: pass --root or set CHAOSBOX_SESSION_CAMPAIGN".to_string())
}

/// Exec `node <tool> <args>`, forwarding the exit code unchanged.
///
/// # Errors
///
/// Returns a message when the interpreter cannot be started; a tool that
/// runs and fails reports through its own exit code.
pub fn exec_node(tool: &Path, args: &[String]) -> Result<i32, String> {
    let status = Command::new("node")
        .arg(tool)
        .args(args)
        .status()
        .map_err(|error| format!("cannot start {}: {error}", tool.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Streaming SHA-256 over a file.
pub(crate) fn file_sha256(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
