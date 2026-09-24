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

/// Extraction failures: filesystem IO, unsupported files, or an invalid
/// explicit source scope.
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("io: {0}")]
    /// Filesystem read error.
    Io(String),
    #[error("unsupported file reported, not interpreted: {0}")]
    /// A file whose format is reported, never silently interpreted or omitted.
    Unsupported(String),
    #[error("invalid source scope: {0}")]
    /// An explicit `sourcePaths` entry that is not a relative in-tree
    /// directory, or a scope root that cannot be entered safely.
    Scope(String),
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
    /// Deterministic `snap:<hex>` identity over repo + scope + file hashes.
    /// Scope is part of the identity: the same file set under a different
    /// explicit scope mints a different id, so changing scope changes the
    /// snapshot (and the run identity derived from it) even when the scoped
    /// directory happens to be empty. `status` surfaces the snapshot ids,
    /// so the scope change is observable there as a fingerprint change.
    pub id: String,
    /// Repository name.
    pub repo: String,
    /// Explicit source scope, repository-relative, sorted and deduped.
    /// Empty means the whole tree (subject to the repository boundary
    /// below). Non-empty restricts capture to those subtrees so pointing
    /// at a workspace root cannot silently pull sibling checkouts.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Pinned file versions in deterministic path order.
    pub files: Vec<FileVersion>,
    /// File texts by repository-relative path.
    pub contents: BTreeMap<String, String>,
}

/// Validate explicit source scope entries (Graphify `sourcePaths` parity).
/// Returns the sorted, deduped scope. Rejects absolute paths, backslashes,
/// empty entries, `.`/`..` segments, and empty segments (no trailing
/// slashes after normalization), so a scope can never escape the corpus
/// root it is resolved against.
pub fn validate_scope(paths: &[String]) -> Result<Vec<String>, ExtractError> {
    let mut out: Vec<String> = Vec::with_capacity(paths.len());
    for raw in paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ExtractError::Scope("empty source path".into()));
        }
        if trimmed.starts_with('/') || trimmed.contains('\\') {
            return Err(ExtractError::Scope(format!(
                "source path must be repository-relative with `/` separators: {raw:?}"
            )));
        }
        // Normalize one trailing slash for operator convenience (`cli/`).
        let normalized = trimmed.trim_end_matches('/').trim();
        if normalized.is_empty() {
            return Err(ExtractError::Scope(format!(
                "source path is empty after normalization: {raw:?}"
            )));
        }
        let mut parts: Vec<&str> = Vec::new();
        for seg in normalized.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(ExtractError::Scope(format!(
                    "source path must not contain empty, `.`, or `..` segments: {raw:?}"
                )));
            }
            parts.push(seg);
        }
        out.push(parts.join("/"));
    }
    out.sort();
    out.dedup();
    Ok(out)
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
    ///
    /// Whole-tree form of [`Snapshot::capture_scoped`] with an empty scope.
    pub fn capture(repo: &str, root: &Path) -> Result<Self, ExtractError> {
        Self::capture_scoped(repo, root, &[])
    }

    /// Walk only `scope` subtrees of `root` (Graphify `sourcePaths` parity).
    ///
    /// An empty scope captures the whole tree exactly like [`Snapshot::capture`].
    /// A non-empty scope restricts capture to those repository-relative
    /// directories: pointing at a workspace root with an explicit scope
    /// cannot silently pull sibling checkouts, because files outside the
    /// scope never enter the snapshot, the extraction, or (via candidates)
    /// inference. Scope is validated by [`validate_scope`] and stored
    /// sorted on the snapshot; it participates in the snapshot id, so a
    /// scope change is a different snapshot even when the file set is
    /// unchanged.
    ///
    /// Fails loudly when a scope entry is invalid, missing, not a
    /// directory, a symlink (no escape from the corpus root), or itself a
    /// nested repository boundary: a scope that cannot be entered safely
    /// must never publish a silently narrowed graph.
    // Long ingestion boundary; splitting stages apart is the owning
    // session's refactor. Allowed to keep CI unblocked.
    #[allow(clippy::too_many_lines)]
    pub fn capture_scoped(repo: &str, root: &Path, scope: &[String]) -> Result<Self, ExtractError> {
        let scope = validate_scope(scope)?;
        // Resolve scope roots up front: every entry must exist, be a
        // directory, and not itself be a symlink or a nested repository.
        // Checking before walking keeps a half-applied scope from ever
        // publishing an incomplete boundary.
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        if scope.is_empty() {
            roots.push(root.to_owned());
        } else {
            for prefix in &scope {
                let dir = root.join(prefix);
                let meta = std::fs::symlink_metadata(&dir).map_err(|e| {
                    ExtractError::Scope(format!("source path {prefix:?} unreadable: {e}"))
                })?;
                if meta.file_type().is_symlink() {
                    return Err(ExtractError::Scope(format!(
                        "source path {prefix:?} is a symlink; scope roots must be real directories"
                    )));
                }
                if !meta.is_dir() {
                    return Err(ExtractError::Scope(format!(
                        "source path {prefix:?} is not a directory"
                    )));
                }
                if dir.join(".git").exists() {
                    return Err(ExtractError::Scope(format!(
                        "source path {prefix:?} is itself a nested repository boundary"
                    )));
                }
                roots.push(dir);
            }
        }
        let mut files = Vec::new();
        let mut contents = BTreeMap::new();
        // Git-aware walk (ripgrep's `ignore` semantics): .gitignore,
        // .git/info/exclude, and global excludes apply even outside a git
        // checkout (`require_git(false)`), so ignored or generated files
        // never enter the snapshot or reach inference. Hidden files are
        // still traversed (`hidden(false)`); only the build-output
        // directories below are pruned on top of ignore rules.
        for walk_root in &roots {
            let walker = ignore::WalkBuilder::new(walk_root)
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
                // A scoped walk rooted at `root/<prefix>` can only produce
                // paths under that prefix; anything else is a walk bug, not
                // a file to index.
                if !scope.is_empty()
                    && !scope
                        .iter()
                        .any(|s| rel == *s || rel.starts_with(&format!("{s}/")))
                {
                    continue;
                }
                if is_supported(&rel) {
                    // Overlapping scopes (e.g. `a` + `a/b`) visit one file
                    // twice: keep the first copy so the snapshot stays a set.
                    if contents.contains_key(&rel) {
                        continue;
                    }
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
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let id = deterministic_id(
            "snap",
            &[
                repo,
                &scope.join(","),
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
            scope,
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
