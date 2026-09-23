//! File-type dispatch, span plumbing, and the per-language extractors.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chaosbox_core::{Entity, EntityKind, SourceSpan};
use regex::Regex;
use serde::{Deserialize, Serialize};

pub(crate) fn is_supported(path: &str) -> bool {
    const EXTS: [&str; 12] = [
        "rs", "py", "js", "ts", "jsx", "tsx", "mjs", "cjs", "nix", "md", "markdown", "txt",
    ];
    match path.rsplit('.').next() {
        // Case-insensitive like the extractor below; a lone ".md" still
        // yields "md" here, preserving the previous matching behavior.
        Some(ext) => EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

/// Unsupported binary/media/document formats: reported, never interpreted.
#[must_use]
pub fn report_unsupported(root: &Path) -> Vec<String> {
    const BAD: [&str; 12] = [
        "png", "jpg", "jpeg", "gif", "mp3", "mp4", "wav", "pdf", "docx", "xlsx", "pptx", "exe",
    ];
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                if BAD.contains(&ext.to_lowercase().as_str()) {
                    out.push(p.to_string_lossy().to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Deterministic extraction output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Extraction {
    /// All entities found, in deterministic file order.
    pub entities: Vec<Entity>,
    /// Explicit `(from_id, to_id, kind)` structural references.
    pub explicit_refs: Vec<(String, String, String)>,
}

fn line_col(text: &str, byte: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in text.char_indices() {
        if i >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

pub(crate) fn span_of(text: &str, file: &str, byte_start: usize, byte_end: usize) -> SourceSpan {
    let (sl, sc) = line_col(text, byte_start);
    let (el, ec) = line_col(text, byte_end.max(byte_start));
    SourceSpan {
        file: file.to_owned(),
        start_line: sl,
        start_col: sc,
        end_line: el,
        end_col: ec,
        byte_start: u32::try_from(byte_start).expect("source byte offset fits in u32"),
        byte_end: u32::try_from(byte_end).expect("source byte offset fits in u32"),
    }
}

/// Extract one file deterministically.
#[must_use]
pub fn extract_file(repo: &str, snapshot: &str, path: &str, text: &str) -> Extraction {
    let mut entities = Vec::new();
    let mut refs = Vec::new();
    let file_entity = Entity::new(
        EntityKind::File,
        repo,
        snapshot,
        path,
        path,
        path,
        span_of(text, path, 0, 0),
    );
    let file_id = file_entity.id.clone();
    entities.push(file_entity);

    let ext_lower = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext_lower == "md" || ext_lower == "markdown" {
        extract_markdown(
            repo,
            snapshot,
            path,
            text,
            &file_id,
            &mut entities,
            &mut refs,
        );
    } else if ext_lower == "txt" {
        let sym = Entity::new(
            EntityKind::Symbol,
            repo,
            snapshot,
            path,
            "text",
            &format!("{path}::text"),
            span_of(text, path, 0, 0),
        );
        refs.push((file_id, sym.id.clone(), "contains".into()));
        entities.push(sym);
    } else if ext_lower == "nix" {
        extract_nix(
            repo,
            snapshot,
            path,
            text,
            &file_id,
            &mut entities,
            &mut refs,
        );
    } else {
        extract_code(
            repo,
            snapshot,
            path,
            text,
            &file_id,
            &mut entities,
            &mut refs,
        );
    }
    Extraction {
        entities,
        explicit_refs: refs,
    }
}

fn extract_code(
    repo: &str,
    snapshot: &str,
    path: &str,
    text: &str,
    file_id: &str,
    entities: &mut Vec<Entity>,
    refs: &mut Vec<(String, String, String)>,
) {
    // Definitions: fn/class/def/interface/type + headings for structure.
    let def_re = Regex::new(r"(?m)^\s*(?:pub\s+)?(?:async\s+)?(?:fn|class|def|interface|type|struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    // Imports: use / import / from / require / export-from.
    let imp_re = Regex::new(r#"(?m)^\s*(?:use\s+([^;]+);|import\s+(?:[^'"]*from\s+)?['"]([^'"]+)['"]|from\s+(\S+)\s+import|require\(\s*['"]([^'"]+)['"]\s*\))"#).unwrap();
    let mut defined: Vec<(String, String)> = Vec::new();
    for cap in def_re.captures_iter(text) {
        let name = cap.get(1).unwrap().as_str();
        let m = cap.get(1).unwrap();
        let qn = format!("{path}::{name}");
        let e = Entity::new(
            EntityKind::Definition,
            repo,
            snapshot,
            path,
            name,
            &qn,
            span_of(text, path, m.start(), m.end()),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "defines".into()));
        defined.push((name.to_owned(), e.id.clone()));
        entities.push(e);
    }
    // Module record for code files.
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    let mod_ent = Entity::new(
        EntityKind::Module,
        repo,
        snapshot,
        path,
        stem,
        &format!("{path}::{stem}"),
        span_of(text, path, 0, 0),
    );
    refs.push((file_id.to_owned(), mod_ent.id.clone(), "contains".into()));
    for (_, did) in &defined {
        refs.push((mod_ent.id.clone(), did.clone(), "defines".into()));
    }
    entities.push(mod_ent);
    for cap in imp_re.captures_iter(text) {
        let target = [cap.get(1), cap.get(2), cap.get(3), cap.get(4)]
            .into_iter()
            .flatten()
            .next()
            .map(|m| m.as_str().trim().to_owned())
            .unwrap_or_default();
        if target.is_empty() {
            continue;
        }
        let m = cap.get(0).unwrap();
        let e = Entity::new(
            EntityKind::Import,
            repo,
            snapshot,
            path,
            &target,
            &format!("{path}::import::{target}"),
            span_of(text, path, m.start(), m.end()),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "imports".into()));
        entities.push(e);
    }
    // Explicit textual references: defined names mentioned elsewhere (bounded:
    // only names defined in this file, not the cartesian product).
    for (name, did) in &defined {
        let pat = format!(r"\b{}\b", regex::escape(name));
        let Ok(re) = Regex::new(&pat) else { continue };
        let mut count = 0;
        for m in re.find_iter(text) {
            count += 1;
            if count > 20 {
                break; // bound per-name mentions
            }
            let _ = m;
        }
        if count > 1 {
            refs.push((file_id.to_owned(), did.clone(), "references".into()));
        }
    }
}

/// Nix expressions: bindings and functions become definitions, relative
/// `.nix` paths become imports, and interpolation/`inherit` occurrences
/// become references to bindings defined in the same file.
///
/// Regex-based and parse-only (no Nix evaluation, no attribute-set or
/// module-system semantics — no completeness claims), ported from
/// graphify's `extractors/nix.py` into the chaosbox entity model:
/// one definition per name (first site wins; a name may rebind in later
/// scopes and the qualified name would collide), bounded reference dedup
/// per (file, binding). Cross-file import resolution happens afterwards
/// in [`extract_snapshot`].
fn extract_nix(
    repo: &str,
    snapshot: &str,
    path: &str,
    text: &str,
    file_id: &str,
    entities: &mut Vec<Entity>,
    refs: &mut Vec<(String, String, String)>,
) {
    // Bindings: `name = ...` at any indent (statement/attribute position).
    // Line-bound (`[ \t]`, not `\s`) so a match can never swallow the next
    // line. Keywords are excluded for graphify parity (`let x = ..` never
    // matches anyway: the token after the ident is not `=`).
    let binding_re = Regex::new(r"(?m)^[ \t]*([A-Za-z_][A-Za-z0-9_'-]*)[ \t]*=").unwrap();
    // Relative import paths: `./x.nix`, `../lib/y.nix`. Graphify's
    // look-around boundaries are enforced by hand below: the `regex` crate
    // has no look-around.
    let import_re = Regex::new(r"((?:\.\.?/)[A-Za-z0-9_./'-]+\.nix)").unwrap();
    // Interpolation: `${name}`.
    let interpolation_re = Regex::new(r"\$\{\s*([A-Za-z_][A-Za-z0-9_'-]*)").unwrap();
    // Inherit lists: `inherit a b;` / `inherit (from) a b;`.
    let inherit_re = Regex::new(r"(?m)\binherit(?:From)?\s+([^;\n}]+)").unwrap();
    let ident_re = Regex::new(r"[A-Za-z_][A-Za-z0-9_'-]*").unwrap();

    // Module record for code files (parity with `extract_code`).
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    let mod_ent = Entity::new(
        EntityKind::Module,
        repo,
        snapshot,
        path,
        stem,
        &format!("{path}::{stem}"),
        span_of(text, path, 0, 0),
    );
    refs.push((file_id.to_owned(), mod_ent.id.clone(), "contains".into()));

    // Bindings and functions -> definitions. First site wins: the same
    // name may bind in several scopes and the qualified name collides.
    let mut defined: BTreeMap<String, String> = BTreeMap::new();
    for cap in binding_re.captures_iter(text) {
        let name = cap.get(1).unwrap();
        if matches!(
            name.as_str(),
            "let" | "in" | "with" | "inherit" | "assert" | "rec"
        ) {
            continue;
        }
        if defined.contains_key(name.as_str()) {
            continue;
        }
        let whole = cap.get(0).unwrap();
        let qn = format!("{path}::{}", name.as_str());
        let e = Entity::new(
            EntityKind::Definition,
            repo,
            snapshot,
            path,
            name.as_str(),
            &qn,
            span_of(text, path, whole.start(), whole.end()),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "defines".into()));
        refs.push((mod_ent.id.clone(), e.id.clone(), "defines".into()));
        defined.insert(name.as_str().to_owned(), e.id.clone());
        entities.push(e);
    }
    entities.push(mod_ent);

    // Relative imports (stubs; resolved to the real target file when the
    // target is part of the snapshot — see `resolve_nix_imports`). The
    // preceding/following character checks replicate graphify's
    // `(?<![A-Za-z0-9_.])` / `(?![A-Za-z0-9_])` boundaries; a rejected
    // boundary rescans from just past the rejected start, so it never hides
    // a later valid match (what Python's finditer + look-around does).
    let mut search = 0usize;
    while let Some(m) = import_re.find(&text[search..]) {
        let start = search + m.start();
        let end = search + m.end();
        let prev_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'));
        let next_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if !(prev_ok && next_ok) {
            search = start + 1;
            continue;
        }
        search = end;
        let target = m.as_str();
        let e = Entity::new(
            EntityKind::Import,
            repo,
            snapshot,
            path,
            target,
            &format!("{path}::import::{target}"),
            span_of(text, path, start, end),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "imports".into()));
        entities.push(e);
    }

    // Interpolation + inherit references: one bounded reference per
    // (file, binding), only for bindings this file defines.
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for cap in interpolation_re.captures_iter(text) {
        referenced.insert(cap.get(1).unwrap().as_str());
    }
    for cap in inherit_re.captures_iter(text) {
        for m in ident_re.find_iter(cap.get(1).unwrap().as_str()) {
            referenced.insert(m.as_str());
        }
    }
    for name in referenced {
        if let Some(did) = defined.get(name) {
            refs.push((file_id.to_owned(), did.clone(), "references".into()));
        }
    }
}

fn extract_markdown(
    repo: &str,
    snapshot: &str,
    path: &str,
    text: &str,
    file_id: &str,
    entities: &mut Vec<Entity>,
    refs: &mut Vec<(String, String, String)>,
) {
    let head_re = Regex::new(r"(?m)^(#{1,6})\s+(.+)$").unwrap();
    let link_re = Regex::new(r"\[([^\]]*)\]\(([^)]+)\)").unwrap();
    let code_re = Regex::new(r"`([^`]+)`").unwrap();
    for cap in head_re.captures_iter(text) {
        let title = cap.get(2).unwrap();
        let e = Entity::new(
            EntityKind::Heading,
            repo,
            snapshot,
            path,
            title.as_str(),
            &format!("{path}#{}", title.as_str()),
            span_of(text, path, title.start(), title.end()),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "contains".into()));
        entities.push(e);
    }
    for cap in link_re.captures_iter(text) {
        let (label, target) = (cap.get(1).unwrap(), cap.get(2).unwrap());
        let e = Entity::new(
            EntityKind::Link,
            repo,
            snapshot,
            path,
            label.as_str(),
            &format!("{path}::link::{}", target.as_str()),
            span_of(text, path, target.start(), target.end()),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "linksto".into()));
        entities.push(e);
    }
    for cap in code_re.captures_iter(text) {
        let inner = cap.get(1).unwrap();
        let e = Entity::new(
            EntityKind::CodeMention,
            repo,
            snapshot,
            path,
            inner.as_str(),
            &format!("{path}::code::{}", inner.as_str()),
            span_of(text, path, inner.start(), inner.end()),
        );
        refs.push((file_id.to_owned(), e.id.clone(), "mentions".into()));
        entities.push(e);
    }
}
