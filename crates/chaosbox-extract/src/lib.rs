//! Deterministic source parsing and candidate generation.
//!
//! Supported (explicit, no complete-call-resolution claims):
//! - Rust / Python / JavaScript+TypeScript: files, modules, symbols,
//!   definitions, imports, containment, explicit textual references.
//! - Nix: bindings/functions as definitions, relative `.nix` imports
//!   (resolved to the target file when it is part of the snapshot),
//!   interpolation/inherit references to in-file bindings (regex-based,
//!   parse-only; no attribute-set or module-system evaluation).
//! - Markdown: headings, links, code mentions, source spans.
//! - Plain text: file/symbol records, lexical mentions.
//!   Unsupported images/audio/video and office docs are reported, never
//!   silently interpreted or omitted.

use std::{collections::BTreeMap, path::Path};

use chaosbox_core::{deterministic_id, sha256_hex};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod candidates;
mod extractors;
mod paths;

#[cfg(test)]
mod tests;

pub use candidates::{CandidateCatalog, build_candidates};
pub use extractors::{Extraction, extract_file, report_unsupported};

use crate::extractors::is_supported;
use crate::paths::resolve_nix_imports;

/// Extraction failures: filesystem IO, or an unsupported file that was
/// reported rather than interpreted.
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("io: {0}")]
    /// Filesystem read error.
    Io(String),
    #[error("unsupported file reported, not interpreted: {0}")]
    /// A file whose format is reported, never silently interpreted or omitted.
    Unsupported(String),
}

/// One source file at a pinned version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVersion {
    /// Repository-relative path.
    pub path: String,
    /// SHA-256 hex of the file text.
    pub sha256: String,
    /// File size in bytes.
    pub bytes: u64,
}

/// A snapshot of a fixture repository (content-addressed).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// Deterministic `snap:<hex>` identity over repo + file hashes.
    pub id: String,
    /// Repository name.
    pub repo: String,
    /// Pinned file versions in deterministic path order.
    pub files: Vec<FileVersion>,
    /// File texts by repository-relative path.
    pub contents: BTreeMap<String, String>,
}

impl Snapshot {
    /// Walk `root`, read supported text files, hash each. Deterministic order.
    ///
    /// Repository boundary: symlinks are never followed (no escape, no
    /// cycles, dangling links skipped), nested repositories (directories
    /// containing `.git`) are not entered, and Git ignore rules
    /// (.gitignore, .git/info/exclude, global excludes) are honored even
    /// outside a git checkout. Indexing a workspace root must not leak
    /// sibling checkouts, generated trees, or ignored files into the graph
    /// or (via candidates) to inference.
    pub fn capture(repo: &str, root: &Path) -> Result<Self, ExtractError> {
        let mut files = Vec::new();
        let mut contents = BTreeMap::new();
        // Git-aware walk (ripgrep's `ignore` semantics): .gitignore,
        // .git/info/exclude, and global excludes apply even outside a git
        // checkout (`require_git(false)`), so ignored or generated files
        // never enter the snapshot or reach inference. Hidden files are
        // still traversed (`hidden(false)`); only the build-output
        // directories below are pruned on top of ignore rules.
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .require_git(false)
            .follow_links(false)
            .sort_by_file_path(std::cmp::Ord::cmp)
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let ft = e.file_type();
                // Never follow symlinks: no escape from the corpus root and
                // no cycles. Symlinked content is out of scope for indexing.
                if ft.is_some_and(|t| t.is_symlink()) {
                    return false;
                }
                let name = e.file_name().to_str().unwrap_or("");
                if name == ".git" || name == "target" || name == "node_modules" {
                    return false;
                }
                // Nested repository boundary.
                if ft.is_some_and(|t| t.is_dir()) && e.path().join(".git").exists() {
                    return false;
                }
                true
            })
            .build();
        for entry in walker {
            let entry = entry.map_err(|e| ExtractError::Io(e.to_string()))?;
            // Surface ignore-file errors: a partially applied exclusion
            // policy must fail loudly, never publish an incomplete boundary.
            if let Some(e) = entry.error() {
                return Err(ExtractError::Io(e.to_string()));
            }
            let p = entry.path();
            // Only regular files are read: directories structure the walk,
            // symlinks are already filtered above, and anything else
            // (pipes, sockets, devices) must never block a read.
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .map_err(|e| ExtractError::Io(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if is_supported(&rel) {
                let text =
                    std::fs::read_to_string(p).map_err(|e| ExtractError::Io(e.to_string()))?;
                let sha = sha256_hex(&[&text]);
                files.push(FileVersion {
                    path: rel.clone(),
                    sha256: sha,
                    bytes: text.len() as u64,
                });
                contents.insert(rel, text);
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let id = deterministic_id(
            "snap",
            &[
                repo,
                &files
                    .iter()
                    .map(|f| format!("{}:{}", f.path, f.sha256))
                    .collect::<Vec<_>>()
                    .join(","),
            ],
        );
        Ok(Self {
            id,
            repo: repo.to_owned(),
            files,
            contents,
        })
    }

    /// Look up a pinned file version by repository-relative path.
    #[must_use]
    pub fn file_version(&self, path: &str) -> Option<&FileVersion> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Content identities for store registration (snapshot id verified).
    #[must_use]
    pub fn snapshot_files(&self) -> Vec<chaosbox_core::SnapshotFile> {
        self.files
            .iter()
            .map(|f| chaosbox_core::SnapshotFile {
                snapshot: self.id.clone(),
                path: f.path.clone(),
                sha256: f.sha256.clone(),
                bytes: f.bytes,
            })
            .collect()
    }
}

/// Extract a whole snapshot.
#[must_use]
pub fn extract_snapshot(snapshot: &Snapshot) -> Extraction {
    let mut entities = Vec::new();
    let mut refs = Vec::new();
    let mut paths: Vec<&String> = snapshot.contents.keys().collect();
    paths.sort();
    for path in paths {
        let text = &snapshot.contents[path];
        let one = extract_file(&snapshot.repo, &snapshot.id, path, text);
        entities.extend(one.entities);
        refs.extend(one.explicit_refs);
    }
    resolve_nix_imports(snapshot, &mut entities, &mut refs);
    Extraction {
        entities,
        explicit_refs: refs,
    }
}
