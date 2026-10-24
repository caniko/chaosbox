//! Deterministic source parsing and candidate generation.
//!
//! Supported (explicit, no complete-call-resolution claims):
//! - Rust / TypeScript / TSX / MTS / CTS / Nix: parser-certified source
//!   declarations and file/module containment, exact spans and syntax coverage.
//!   Rust/TS also certify syntactic nesting and marked rationale comments.
//!   Rust/TS call and heritage sites resolve to a *unique* same-language
//!   definition as INFERRED observations (`unique-name-v1`); ADR/RFC
//!   citations resolve to captured documents (`citations-v1`).
//!   Imports and other lexical name matches remain uncertified proposals.
//!   Nix relative `.nix` path proposals resolve to captured file targets;
//!   no compiler, scope, attribute-set or module-system evaluation is claimed.
//! - Python / JavaScript: heuristic definitions/imports/textual references.
//! - Markdown: headings, links, code mentions, source spans.
//! - Plain text: file/symbol records, lexical mentions.
//!   Unsupported images/audio/video and office docs are reported, never
//!   silently interpreted or omitted.

use std::{collections::BTreeMap, path::Path};

use chaosbox_core::{deterministic_id, sha256_hex};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod candidates;
pub mod compiler;
mod extractors;
mod inventory;
mod paths;
mod resolve;
mod selection;
mod syntax;

#[cfg(test)]
mod tests;

pub use candidates::{CandidateCatalog, build_candidates};
pub use extractors::{Extraction, StructuralFact, extract_file, report_unsupported};
pub use inventory::{InventoryLimits, SourceInventory, source_inventory};

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
    /// Selected files excluded because they are not valid UTF-8. They are
    /// never interpreted (like unsupported extensions) and never enter the
    /// snapshot identity; listed so the omission stays observable.
    #[serde(default)]
    pub unreadable: Vec<String>,
}

/// Validate explicit repository-relative source scope entries.
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

/// Validate exclusion globs (gitignore syntax, repository-relative).
/// Returns them sorted and deduped. Rejects empty patterns, negations (`!`
/// would re-include and invert intent), backslashes, absolute paths and
/// `..` segments.
pub fn validate_exclude(patterns: &[String]) -> Result<Vec<String>, ExtractError> {
    let mut out = Vec::with_capacity(patterns.len());
    for raw in patterns {
        let pattern = raw.trim();
        if pattern.is_empty()
            || pattern.starts_with('!')
            || pattern.starts_with('/')
            || pattern.contains('\\')
            || pattern.split('/').any(|seg| seg == "..")
        {
            return Err(ExtractError::Scope(format!(
                "exclude must be a non-empty, non-negated, repository-relative glob: {raw:?}"
            )));
        }
        out.push(pattern.to_owned());
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

    /// Walk only explicitly selected `scope` subtrees of `root`.
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
    pub fn capture_scoped(repo: &str, root: &Path, scope: &[String]) -> Result<Self, ExtractError> {
        Self::capture_excluding(repo, root, scope, &[])
    }

    /// [`Snapshot::capture_scoped`] minus files matching `exclude`
    /// (gitignore-syntax globs, see [`validate_exclude`]). A `.chaosboxignore`
    /// file is always honored like `.gitignore`. Non-empty exclusions are
    /// part of the snapshot id, so narrowing a capture is never silent.
    pub fn capture_excluding(
        repo: &str,
        root: &Path,
        scope: &[String],
        exclude: &[String],
    ) -> Result<Self, ExtractError> {
        Self::capture_matching(repo, root, scope, exclude, is_supported)
    }

    pub(crate) fn capture_matching(
        repo: &str,
        root: &Path,
        scope: &[String],
        exclude: &[String],
        include: fn(&str) -> bool,
    ) -> Result<Self, ExtractError> {
        let exclude = validate_exclude(exclude)?;
        let mut files = Vec::new();
        let mut contents = BTreeMap::new();
        let mut unreadable = Vec::new();
        let selection =
            inventory::visit_selected(root, scope, &exclude, include, None, |path, relative| {
                let bytes = std::fs::read(path).map_err(|e| ExtractError::Io(e.to_string()))?;
                let Ok(text) = String::from_utf8(bytes) else {
                    // Binary bytes under a source extension (one such file used
                    // to fail whole repository captures). Skip and report; a
                    // parser can certify nothing about it anyway.
                    unreadable.push(relative.to_owned());
                    return Ok(());
                };
                files.push(FileVersion {
                    path: relative.to_owned(),
                    sha256: sha256_hex(&[&text]),
                    bytes: text.len() as u64,
                });
                contents.insert(relative.to_owned(), text);
                Ok(())
            })?;
        let scope = selection.scope;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let listing = files
            .iter()
            .map(|f| format!("{}:{}", f.path, f.sha256))
            .collect::<Vec<_>>()
            .join(",");
        // Unrestricted captures keep their historical ids.
        let id = if exclude.is_empty() {
            deterministic_id("snap", &[repo, &scope.join(","), &listing])
        } else {
            let exclude = format!("exclude={}", exclude.join(","));
            deterministic_id("snap", &[repo, &scope.join(","), &exclude, &listing])
        };
        Ok(Self {
            id,
            repo: repo.to_owned(),
            scope,
            files,
            contents,
            unreadable,
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
    let mut facts = Vec::new();
    let mut coverage = Vec::new();
    let mut uses = syntax::Uses::default();
    let mut paths: Vec<&String> = snapshot.contents.keys().collect();
    paths.sort();
    for path in paths {
        let text = &snapshot.contents[path];
        let (one, found) = extractors::extract_with_sites(&snapshot.repo, &snapshot.id, path, text);
        entities.extend(one.entities);
        refs.extend(one.explicit_refs);
        facts.extend(one.facts);
        coverage.extend(one.coverage);
        uses.sites.extend(found.sites);
        uses.callable.extend(found.callable);
        uses.types.extend(found.types);
    }
    resolve_nix_imports(snapshot, &mut entities, &mut refs);
    resolve::names(&uses, &entities, &mut facts);
    resolve::citations(snapshot, &entities, &mut facts);
    Extraction {
        entities,
        explicit_refs: refs,
        facts,
        coverage,
        compiler: None,
    }
}
