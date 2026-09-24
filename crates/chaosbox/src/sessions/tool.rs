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
/// The pin is a closure, not a single file: every entry in `tools.json`
/// is re-hashed on every resolution, so a tampered transitive import fails
/// even when the entrypoint itself is untouched. The walk then starts at the
/// requested tool and follows every relative import it declares — and every
/// relative import *those* files declare — refusing the first hop that lands
/// outside the pinned set. Checking only the entrypoint's direct imports
/// would let an unpinned file three hops down execute unattested.
///
/// # Errors
///
/// Returns a message when the pin record is missing or unreadable, when the
/// tool is not pinned, when any pinned file does not hash to its pin, when
/// any file reachable through relative imports is not pinned, or when a file
/// cannot be read.
pub fn resolve_tool(root: &Path, name: &str) -> Result<PathBuf, String> {
    let pins = root.join("tools.json");
    let text = fs::read_to_string(&pins)
        .map_err(|error| format!("cannot read {}: {error}", pins.display()))?;
    let pinned: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot parse {}: {error}", pins.display()))?;
    let tools = pinned.get("tools").ok_or_else(|| {
        format!(
            "no pinned tool closure in {}: refusing to run",
            pins.display()
        )
    })?;
    // Every pinned file must still hash to its pin: the closure is verified
    // as a whole so a changed `lib.mjs` cannot hide behind an unchanged
    // `install.mjs`.
    let entries = tools.as_object().ok_or_else(|| {
        format!(
            "pinned tool closure in {} is not an object: refusing to run",
            pins.display()
        )
    })?;
    let mut pinned_paths: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for (entry_name, entry) in entries {
        let (Some(path), Some(expected)) = (
            entry.get("path").and_then(Value::as_str),
            entry.get("sha256").and_then(Value::as_str),
        ) else {
            return Err(format!(
                "{entry_name} has no path/digest in {}",
                pins.display()
            ));
        };
        let actual =
            file_sha256(Path::new(path)).map_err(|error| format!("cannot hash {path}: {error}"))?;
        if actual != expected {
            return Err(format!(
                "{entry_name} at {path} digests as {actual} instead of the pinned {expected}: refusing to run"
            ));
        }
        pinned_paths.insert(canonicalize_lexically(Path::new(path)));
    }
    let entry = tools
        .get(name)
        .ok_or_else(|| format!("{name} is not pinned in {}", pins.display()))?;
    let path = entry
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} has no path in {}", pins.display()))?;
    // Transitive closure walk: start at the entrypoint, follow every relative
    // import, and require each hop to be a pinned file whose own imports are
    // then walked in turn. Only `./` and `../` imports can name campaign
    // files; bare specifiers and `node:` builtins never touch the campaign.
    // Comparison is lexical (`./lib.mjs` == `lib.mjs`), so pretty-printing
    // never perturbs the pin. Every file in the walk was hashed above, so a
    // cycle is the only way to revisit one.
    let entry_path = canonicalize_lexically(Path::new(path));
    let mut queue = vec![entry_path.clone()];
    let mut walked: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    while let Some(file) = queue.pop() {
        if !walked.insert(file.clone()) {
            continue;
        }
        let body = fs::read_to_string(&file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let dir = file
            .parent()
            .ok_or_else(|| {
                format!(
                    "{} has no parent directory: refusing to run",
                    file.display()
                )
            })?
            .to_path_buf();
        for import in relative_imports(&body) {
            let resolved = canonicalize_lexically(&dir.join(&import));
            if !pinned_paths.contains(&resolved) {
                let from = if file == entry_path {
                    name.to_string()
                } else {
                    file.display().to_string()
                };
                return Err(format!(
                    "{from} imports {import} which is not pinned in {}: refusing to run",
                    pins.display()
                ));
            }
            queue.push(resolved);
        }
    }
    Ok(PathBuf::from(path))
}

/// Lexical absolute form of a path: `.` and `..` resolved without touching
/// the filesystem, so `./lib.mjs` and `lib.mjs` compare equal. Unlike
/// `canonicalize`, it never fails on missing files — the closure check must
/// refuse an unpinned import even when the file it names does not exist.
fn canonicalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Relative `./` and `../` import specifiers in a JS tool body.
///
/// Scans `from "..."` and `import "..."` forms; anything else (bare
/// specifiers, `node:` builtins, absolute paths) cannot name a campaign file
/// and is ignored by the closure check.
fn relative_imports(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["from \"", "from '", "import \"", "import '"] {
        let mut rest = body;
        while let Some(at) = rest.find(marker) {
            let after = &rest[at + marker.len()..];
            let end = after
                .find('"')
                .zip(after.find('\''))
                .map(|(a, b)| a.min(b))
                .or_else(|| after.find('"'))
                .or_else(|| after.find('\''));
            let Some(end) = end else { break };
            let specifier = &after[..end];
            if specifier.starts_with("./") || specifier.starts_with("../") {
                // Strip query/hash: pins are files, not URLs.
                let file = specifier
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(specifier)
                    .to_string();
                if !found.contains(&file) {
                    found.push(file);
                }
            }
            rest = &after[end + 1..];
        }
    }
    found
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
