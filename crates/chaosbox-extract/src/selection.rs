//! Small, explicitly selected file sets for immutable workspace investigations.

use std::{collections::BTreeMap, io::Read, path::Path};
use chaosbox_core::{deterministic_id, sha256_hex};
use crate::{ExtractError, FileVersion, Snapshot};

impl Snapshot {
    /// Capture exactly the requested regular UTF-8 files (including configuration).
    /// Explicit selection is not an automatic repository inventory. Up to 128
    /// files/16 MiB total; every path is checked against repository boundaries.
    /// File-set snapshots have a distinct identity domain from subtree captures.
    pub fn capture_files(repo: &str, root: &Path, paths: &[String]) -> Result<Self, ExtractError> {
        let root = root
            .canonicalize()
            .map_err(|e| ExtractError::Io(e.to_string()))?;
        if paths.is_empty() || paths.len() > 128 {
            return Err(ExtractError::Scope(
                "file selection must contain 1..128 paths".into(),
            ));
        }
        let mut selected = paths.to_vec();
        selected.sort();
        selected.dedup();
        let mut contents = BTreeMap::new();
        let mut files = Vec::new();
        let mut remaining = 16 * 1024 * 1024_u64;
        for path in &selected {
            let file = crate::paths::checked_path(&root, path, true)?;
            let mut text = String::new();
            std::fs::File::open(file)
                .map_err(|e| ExtractError::Io(e.to_string()))?
                .take(remaining + 1)
                .read_to_string(&mut text)
                .map_err(|e| ExtractError::Io(e.to_string()))?;
            remaining = remaining
                .checked_sub(text.len() as u64)
                .ok_or_else(|| ExtractError::Scope("file selection exceeds 16 MiB".into()))?;
            files.push(FileVersion {
                path: path.clone(),
                sha256: sha256_hex(&[&text]),
                bytes: text.len() as u64,
            });
            contents.insert(path.clone(), text);
        }
        let identity =
            serde_json::to_string(&files).map_err(|e| ExtractError::Io(e.to_string()))?;
        Ok(Self {
            id: deterministic_id("snap", &[repo, "file-set-v1", &identity]),
            repo: repo.into(),
            scope: selected,
            files,
            contents,
        })
    }
}
