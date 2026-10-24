//! Shared source selection for snapshots and bounded, metadata-only admission.

use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{extractors::is_supported, validate_scope, ExtractError};

/// Bounds on a metadata-only source walk. Partial walks never admit capture.
#[derive(Clone, Copy, Debug)]
pub struct InventoryLimits {
    /// Maximum walker entries (including directories and unsupported files).
    pub maximum_entries: u64,
    /// Maximum elapsed walk time; callers should also bound the process lifetime.
    pub timeout: Duration,
}

/// Selected file metadata, without source text, extraction, or fingerprints.
#[derive(Debug, Serialize)]
pub struct SourceInventory {
    /// Validated, sorted source scope, identical to snapshot capture.
    pub scope: Vec<String>,
    /// Number of selected regular files.
    pub files: u64,
    /// Sum of selected file sizes at the time of observation.
    pub bytes: u64,
    /// Walker entries visited, including directories and unsupported files.
    pub entries_visited: u64,
    /// Whether every selected subtree was walked within the bounds.
    pub complete: bool,
    /// Limit that stopped a partial walk.
    pub stopped_by: Option<&'static str>,
}

/// Inspect sizes using snapshot selection, without reading source contents.
///
/// Git ignores, supported extensions, symlinks, nested repositories, hidden
/// files, and overlapping scopes follow the same walker as native capture.
/// This is admission evidence only: it does not establish source freshness.
pub fn source_inventory(
    root: &Path,
    scope: &[String],
    limits: InventoryLimits,
) -> Result<SourceInventory, ExtractError> {
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    // ponytail: admission ignores `--exclude`, so it can over-count a narrowed
    // capture; the bound stays conservative. Thread excludes through if that
    // ever rejects a capture that would fit.
    let mut inventory = visit_selected(root, scope, &[], is_supported, Some(limits), |path, _| {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|e| ExtractError::Io(e.to_string()))?;
        if !metadata.is_file() {
            return Err(ExtractError::Io(
                "selected file changed type during inventory".into(),
            ));
        }
        files += 1;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or_else(|| ExtractError::Io("source size overflow".into()))?;
        Ok(())
    })?;
    inventory.files = files;
    inventory.bytes = bytes;
    Ok(inventory)
}

/// Exclusions are gitignore-syntax globs rooted at the corpus root. With only
/// negated override globs, every other path keeps the normal ignore rules.
fn exclusions(
    root: &Path,
    exclude: &[String],
) -> Result<ignore::overrides::Override, ExtractError> {
    let mut overrides = ignore::overrides::OverrideBuilder::new(root);
    for pattern in exclude {
        overrides
            .add(&format!("!{pattern}"))
            .map_err(|e| ExtractError::Scope(format!("invalid exclude {pattern:?}: {e}")))?;
    }
    overrides
        .build()
        .map_err(|e| ExtractError::Scope(e.to_string()))
}

pub(crate) fn visit_selected(
    root: &Path,
    scope: &[String],
    exclude: &[String],
    include: fn(&str) -> bool,
    limits: Option<InventoryLimits>,
    mut visit: impl FnMut(&Path, &str) -> Result<(), ExtractError>,
) -> Result<SourceInventory, ExtractError> {
    if limits.is_some_and(|limits| limits.maximum_entries == 0 || limits.timeout.is_zero()) {
        return Err(ExtractError::Scope(
            "inventory bounds must be positive".into(),
        ));
    }
    let started = Instant::now();
    let scope = validate_scope(scope)?;
    let mut roots = Vec::new();
    if scope.is_empty() {
        roots.push(root.to_owned());
    } else {
        for prefix in &scope {
            let dir = crate::paths::checked_path(root, prefix, false)?;
            if !dir.is_dir() {
                return Err(ExtractError::Scope(format!(
                    "source path {prefix:?} is not a directory"
                )));
            }
            roots.push(dir);
        }
    }
    let mut result = SourceInventory {
        scope,
        files: 0,
        bytes: 0,
        entries_visited: 0,
        complete: false,
        stopped_by: None,
    };
    let overrides = exclusions(root, exclude)?;
    let mut seen = BTreeSet::new();
    for walk_root in &roots {
        // Preserve native ignore behavior, including parent/global exclusions,
        // outside Git checkouts. The filter is shared by capture and inventory.
        let walker = ignore::WalkBuilder::new(walk_root)
            .add_custom_ignore_filename(".chaosboxignore")
            .overrides(overrides.clone())
            .hidden(false)
            .require_git(false)
            .follow_links(false)
            .sort_by_file_path(std::cmp::Ord::cmp)
            .filter_entry(|entry| {
                if entry.depth() == 0 {
                    return true;
                }
                let kind = entry.file_type();
                if kind.is_some_and(|kind| kind.is_symlink()) {
                    return false;
                }
                let name = entry.file_name().to_str().unwrap_or("");
                if matches!(name, ".git" | "target" | "node_modules") {
                    return false;
                }
                !(kind.is_some_and(|kind| kind.is_dir()) && entry.path().join(".git").exists())
            })
            .build();
        for entry in walker {
            if let Some(limits) = limits {
                let reason = if result.entries_visited >= limits.maximum_entries {
                    Some("maximum-entries")
                } else if started.elapsed() >= limits.timeout {
                    Some("timeout")
                } else {
                    None
                };
                if reason.is_some() {
                    result.stopped_by = reason;
                    return Ok(result);
                }
            }
            result.entries_visited += 1;
            let entry = entry.map_err(|e| ExtractError::Io(e.to_string()))?;
            if let Some(error) = entry.error() {
                return Err(ExtractError::Io(error.to_string()));
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|e| ExtractError::Io(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if !result.scope.is_empty()
                && !result.scope.iter().any(|prefix| {
                    relative == *prefix || relative.starts_with(&format!("{prefix}/"))
                })
            {
                continue;
            }
            if include(&relative) && seen.insert(relative.clone()) {
                visit(path, &relative)?;
            }
        }
    }
    if limits.is_some_and(|limits| started.elapsed() >= limits.timeout) {
        result.stopped_by = Some("timeout");
        return Ok(result);
    }
    result.complete = true;
    Ok(result)
}
