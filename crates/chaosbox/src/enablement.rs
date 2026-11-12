//! Explicit per-project opt-in. Agent surfaces (MCP graph tools, hints,
//! guidance) act only inside a Git checkout whose root holds a committed
//! `.chaosbox.json` naming the graph repository: `{"repo": "<name>"}`.

use std::path::{Path, PathBuf};

/// Marker file name at the checkout root.
pub const MARKER: &str = ".chaosbox.json";

/// An enabled project: its checkout root and graph repository name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enabled {
    pub root: PathBuf,
    pub repo: String,
}

/// Walk up from `dir` to the nearest `.git` (directory or worktree file) and
/// read its marker. `None` when outside a checkout, unmarked, or malformed:
/// opt-in fails closed.
#[must_use]
pub fn enabled(dir: &Path) -> Option<Enabled> {
    let root = dir.ancestors().find(|d| d.join(".git").exists())?;
    let text = std::fs::read_to_string(root.join(MARKER)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let repo = value.get("repo")?.as_str()?.trim();
    (!repo.is_empty()).then(|| Enabled {
        root: root.to_path_buf(),
        repo: repo.to_owned(),
    })
}

/// `enabled` for the process working directory.
#[must_use]
pub fn current() -> Option<Enabled> {
    enabled(&std::env::current_dir().ok()?)
}

#[cfg(test)]
mod tests {
    use super::{enabled, MARKER};

    #[test]
    fn opt_in_requires_marker_at_checkout_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(enabled(&sub), None, "outside a checkout");
        std::fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(enabled(&sub), None, "unmarked checkout");
        std::fs::write(root.join(MARKER), r#"{"repo":" "}"#).unwrap();
        assert_eq!(enabled(&sub), None, "blank repo");
        std::fs::write(root.join(MARKER), "repo = demo").unwrap();
        assert_eq!(enabled(&sub), None, "malformed");
        std::fs::write(root.join(MARKER), r#"{"repo":"demo"}"#).unwrap();
        let on = enabled(&sub).unwrap();
        assert_eq!((on.root.as_path(), on.repo.as_str()), (root, "demo"));
    }
}
