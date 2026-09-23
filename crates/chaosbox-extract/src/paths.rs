//! Repository-relative path resolution for imports.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use chaosbox_core::{Entity, EntityKind};

use crate::Snapshot;
use crate::extractors::span_of;

/// Lexically resolve `target` (a `./`/`../` relative path) against the
/// directory of `from_file`. `None` when the path is absolute or escapes
/// the repository root (an import must never point outside the snapshot).
fn resolve_relative_path(from_file: &str, target: &str) -> Option<String> {
    let mut stack: Vec<String> = Path::new(from_file)
        .parent()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    for c in Path::new(target).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::Normal(s) => stack.push(s.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if stack.is_empty() {
        None
    } else {
        Some(stack.join("/"))
    }
}

/// Resolve relative `.nix` imports against the snapshot's file set: an
/// in-repo target replaces its import stub with an edge to the target's
/// real `File` entity (graphify nix parity: imports land on the file node,
/// never a duplicate stub). A missing or external target keeps the stub so
/// the import stays visible instead of silently vanishing.
pub(crate) fn resolve_nix_imports(
    snapshot: &Snapshot,
    entities: &mut Vec<Entity>,
    refs: &mut Vec<(String, String, String)>,
) {
    let (stubs, resolved) = {
        let file_ids: BTreeMap<&str, &str> = entities
            .iter()
            .filter(|e| e.kind == EntityKind::File)
            .map(|e| (e.file.as_str(), e.id.as_str()))
            .collect();
        let mut stubs: BTreeSet<String> = BTreeSet::new();
        let mut resolved: Vec<(String, String, String)> = Vec::new();
        for e in entities.iter() {
            if e.kind != EntityKind::Import
                || !Path::new(&e.file)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("nix"))
            {
                continue;
            }
            if !(e.name.starts_with("./") || e.name.starts_with("../")) {
                continue;
            }
            let Some(target) = resolve_relative_path(&e.file, &e.name) else {
                continue;
            };
            // Self-import stays a visible stub; `push` would drop the
            // self edge anyway.
            if target == e.file || !snapshot.contents.contains_key(&target) {
                continue;
            }
            let (Some(from_id), Some(target_text)) = (
                file_ids.get(e.file.as_str()),
                snapshot.contents.get(&target),
            ) else {
                continue;
            };
            // Identical construction to `extract_file`'s file entity
            // (byte-0 span), so the id matches the target's own node.
            let target_file = Entity::new(
                EntityKind::File,
                &snapshot.repo,
                &snapshot.id,
                &target,
                &target,
                &target,
                span_of(target_text, &target, 0, 0),
            );
            resolved.push(((*from_id).to_owned(), target_file.id, "imports".into()));
            stubs.insert(e.id.clone());
        }
        (stubs, resolved)
    };
    if stubs.is_empty() {
        return;
    }
    refs.retain(|(f, t, _)| !stubs.contains(f) && !stubs.contains(t));
    entities.retain(|e| !stubs.contains(&e.id));
    refs.extend(resolved);
}
