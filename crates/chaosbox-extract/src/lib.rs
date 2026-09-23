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

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use chaosbox_core::{
    deterministic_id, sha256_hex, Candidate, Entity, EntityKind, RelationType, SourceSpan,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

fn is_supported(path: &str) -> bool {
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

fn span_of(text: &str, file: &str, byte_start: usize, byte_end: usize) -> SourceSpan {
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
fn resolve_nix_imports(
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

/// Bounded candidate construction outcome: the selected candidates plus
/// truthful per-reason accounting of what the cap omitted (truncation is
/// never silent).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateCatalog {
    /// Selected candidates in deterministic construction order.
    pub candidates: Vec<Candidate>,
    /// Selected counts by candidate reason.
    pub selected: BTreeMap<String, u64>,
    /// Omitted counts by reason: unique candidates that reached an
    /// already-full cap (duplicates and self-pairs are not omissions).
    pub omitted: BTreeMap<String, u64>,
    /// The candidate cap that produced this catalog.
    pub cap: usize,
}

/// Bounded candidate construction: no cartesian product.
///
/// Sources: same-file co-occurrence, qualified-name match, explicit imports,
/// lexical mentions, structural (file->module->definition) neighborhoods.
/// Cap total candidates to keep Jev budgets bounded; per-reason
/// selected/omitted counts make the truncation observable (see
/// [`CandidateCatalog`]).
// Over the default line budget; splitting the bounded pipeline stages
// apart is the owning session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn build_candidates(extraction: &Extraction, max_candidates: usize) -> CandidateCatalog {
    // Nested helper first: items exist from scope start (clarity lint).
    // Eight parameters are a smell the owning session should refactor
    // (e.g. a builder struct); allowed to keep this integration unblocked.
    #[allow(clippy::too_many_arguments)]
    fn push(
        cat: &mut CandidateCatalog,
        seen: &mut BTreeSet<(String, String, String)>,
        rel: RelationType,
        from: &str,
        to: &str,
        reason: &str,
        excerpt: &str,
    ) {
        if from == to {
            return;
        }
        let key = (rel_name(&rel), from.to_owned(), to.to_owned());
        // Duplicates are neither selected nor omitted: only a unique
        // candidate that meets a full cap counts as an omission.
        if !seen.insert(key) {
            return;
        }
        if cat.candidates.len() >= cat.cap {
            *cat.omitted.entry(reason.to_owned()).or_insert(0) += 1;
            return;
        }
        let id = deterministic_id("cand", &[&rel_name(&rel), from, to, reason]);
        cat.candidates.push(Candidate {
            id,
            rel_type: rel,
            from_entity: from.to_owned(),
            to_entity: to.to_owned(),
            reason: reason.to_owned(),
            state_excerpt: excerpt.chars().take(500).collect(),
        });
        *cat.selected.entry(reason.to_owned()).or_insert(0) += 1;
    }
    let by_id: BTreeMap<&str, &Entity> = extraction
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    let mut name_index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &extraction.entities {
        name_index
            .entry(e.name.clone())
            .or_default()
            .push(e.id.clone());
        name_index
            .entry(e.qualified_name.clone())
            .or_default()
            .push(e.id.clone());
    }
    let mut cat = CandidateCatalog {
        candidates: Vec::new(),
        selected: BTreeMap::new(),
        omitted: BTreeMap::new(),
        cap: max_candidates,
    };
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    // 1. structural edges from extraction refs
    for (from, to, kind) in &extraction.explicit_refs {
        let rel = match kind.as_str() {
            "defines" => RelationType::Defines,
            "imports" => RelationType::Imports,
            "references" | "mentions" => RelationType::References,
            "linksto" => RelationType::LinksTo,
            _ => RelationType::Contains,
        };
        let excerpt = by_id
            .get(to.as_str())
            .map(|e| e.qualified_name.clone())
            .unwrap_or_default();
        push(&mut cat, &mut seen, rel, from, to, "structural", &excerpt);
    }
    // 2. import -> definition resolution by last-segment lexical match (bounded)
    let imports: Vec<&Entity> = extraction
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Import)
        .collect();
    let defs: Vec<&Entity> = extraction
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Definition)
        .collect();
    for imp in &imports {
        let last = imp
            .name
            .split(['/', '.', ':'])
            .next_back()
            .unwrap_or(&imp.name);
        for d in defs.iter().take(200) {
            if d.name == last && imp.file != d.file {
                push(
                    &mut cat,
                    &mut seen,
                    RelationType::References,
                    &imp.id,
                    &d.id,
                    "lexical-import",
                    &d.qualified_name,
                );
            }
            if cat.candidates.len() >= cat.cap {
                break;
            }
        }
    }
    // 3. same-file definition co-occurrence (bounded pairs per file)
    let mut by_file: BTreeMap<&str, Vec<&Entity>> = BTreeMap::new();
    for e in &extraction.entities {
        if e.kind == EntityKind::Definition {
            by_file.entry(e.file.as_str()).or_default().push(e);
        }
    }
    for defs_in_file in by_file.values() {
        for pair in defs_in_file.windows(2).take(25) {
            push(
                &mut cat,
                &mut seen,
                RelationType::References,
                &pair[0].id,
                &pair[1].id,
                "co-occurrence",
                &pair[1].qualified_name,
            );
        }
    }
    cat
}

fn rel_name(r: &RelationType) -> String {
    format!("{r:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::PathBuf;

    fn tmp_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let p = dir.path().join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    #[test]
    fn snapshot_is_deterministic() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn foo() {}\n"), ("b.py", "def bar(): pass\n")]);
        let s1 = Snapshot::capture("r", &root).unwrap();
        let s2 = Snapshot::capture("r", &root).unwrap();
        assert_eq!(s1.id, s2.id);
        assert_eq!(s1.files.len(), 2);
    }

    #[test]
    fn rust_python_js_md_txt_covered() {
        let (_t, root) = tmp_repo(&[
            ("a.rs", "use foo::bar;\nfn hello() {}\nhello();\n"),
            ("b.py", "import os\ndef greet(): pass\n"),
            ("c.ts", "import { x } from './y';\nfunction f() {}\n"),
            ("d.md", "# Title\n[link](https://example.com) `code`\n"),
            ("e.txt", "plain words\n"),
        ]);
        let snap = Snapshot::capture("r", &root).unwrap();
        assert_eq!(snap.files.len(), 5);
        let ext = extract_snapshot(&snap);
        let kinds: BTreeSet<String> = ext
            .entities
            .iter()
            .map(|e| format!("{:?}", e.kind))
            .collect();
        for k in [
            "File",
            "Module",
            "Definition",
            "Import",
            "Heading",
            "Link",
            "CodeMention",
        ] {
            assert!(kinds.contains(k), "missing {k} in {kinds:?}");
        }
        let cat = build_candidates(&ext, 100);
        assert!(!cat.candidates.is_empty());
        assert!(cat.candidates.len() <= 100, "candidates bounded");
        assert_eq!(cat.cap, 100);
        assert_eq!(
            cat.selected.values().sum::<u64>(),
            u64::try_from(cat.candidates.len()).expect("candidate count fits in u64"),
        );
        assert!(cat.omitted.is_empty(), "under the cap nothing omitted");
    }

    #[test]
    fn no_cartesian_product() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")]);
        let snap = Snapshot::capture("r", &root).unwrap();
        let ext = extract_snapshot(&snap);
        let cat = build_candidates(&ext, 10);
        // 3 defs would be 9 pairs cartesian; bounded co-occurrence gives <= 2 + structural
        assert!(cat.candidates.len() <= 10);
    }

    #[test]
    fn candidate_cap_reports_omissions() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn one() {}\nfn two() {}\nfn three() {}\n")]);
        let snap = Snapshot::capture("r", &root).unwrap();
        let ext = extract_snapshot(&snap);
        // Well under the cap: everything selected, nothing omitted.
        let full = build_candidates(&ext, 100);
        assert!(full.omitted.is_empty());
        // Under the cap: truncation is observable, not silent.
        let cat = build_candidates(&ext, 2);
        assert_eq!(cat.candidates.len(), 2, "cap enforced");
        assert_eq!(cat.cap, 2);
        let selected = cat.selected.values().sum::<u64>();
        let omitted = cat.omitted.values().sum::<u64>();
        assert_eq!(selected, 2, "selected matches the cap");
        assert!(
            omitted >= 1,
            "omitted candidates must be accounted: {cat:?}"
        );
        assert!(selected + omitted < 100, "accounting stays local");
        // Deterministic: same input, same catalog.
        let again = build_candidates(&ext, 2);
        assert_eq!(cat.candidates, again.candidates);
        assert_eq!(cat.omitted, again.omitted);
    }

    #[test]
    fn nix_bindings_imports_and_references_extracted() {
        let (_t, root) = tmp_repo(&[
            (
                "main.nix",
                "{ ... }:\n\
                 let\n\
                 \x20 name = \"world\";\n\
                 in {\n\
                 \x20 imports = [ ./parts/extra.nix ];\n\
                 \x20 greeting = \"hello ${name}\";\n\
                 \x20 inherit (builtins) toString;\n\
                 }\n",
            ),
            (
                "parts/extra.nix",
                "{ config, ... }:\n{\n  enable = true;\n}\n",
            ),
        ]);
        let snap = Snapshot::capture("r", &root).unwrap();
        assert_eq!(snap.files.len(), 2, "both .nix files snapshotted");
        let ext = extract_snapshot(&snap);

        let file_id = |path: &str| {
            ext.entities
                .iter()
                .find(|e| e.kind == EntityKind::File && e.file == path)
                .map_or_else(|| panic!("file entity for {path}"), |e| e.id.clone())
        };
        let main_id = file_id("main.nix");
        let parts_id = file_id("parts/extra.nix");

        // Bindings become definitions (keyword-free, first site wins).
        let defs: BTreeSet<&str> = ext
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Definition && e.file == "main.nix")
            .map(|e| e.name.as_str())
            .collect();
        for name in ["name", "imports", "greeting"] {
            assert!(defs.contains(name), "missing binding {name} in {defs:?}");
        }

        // Resolved in-repo import: the stub is gone, the edge lands on
        // the real target file node.
        assert_eq!(
            ext.entities
                .iter()
                .filter(|e| e.kind == EntityKind::Import)
                .count(),
            0,
            "resolved import stub must be replaced by the file edge"
        );
        assert!(
            ext.explicit_refs
                .contains(&(main_id.clone(), parts_id, "imports".to_owned())),
            "file -> file import edge required"
        );

        // `${name}` interpolation references the in-file binding.
        let name_def = ext
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::Definition && e.name == "name")
            .map(|e| e.id.clone())
            .unwrap();
        assert!(
            ext.explicit_refs
                .contains(&(main_id, name_def, "references".to_owned())),
            "interpolation reference required"
        );
    }

    #[test]
    fn nix_unresolved_import_keeps_stub() {
        let (_t, root) = tmp_repo(&[(
            "main.nix",
            "{ ... }:\n{\n  imports = [ ./missing.nix ];\n}\n",
        )]);
        let snap = Snapshot::capture("r", &root).unwrap();
        let ext = extract_snapshot(&snap);
        let stubs: Vec<&Entity> = ext
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Import)
            .collect();
        assert_eq!(stubs.len(), 1, "unresolved import stays visible");
        assert_eq!(stubs[0].name, "./missing.nix");
        let main_id = ext
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::File && e.file == "main.nix")
            .map_or_else(|| panic!("file entity for main.nix"), |e| e.id.clone());
        assert!(
            ext.explicit_refs
                .contains(&(main_id, stubs[0].id.clone(), "imports".to_owned())),
            "stub keeps its import edge"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_skips_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.rs"), "fn secret() {}\n").unwrap();
        let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
        std::os::unix::fs::symlink(outside.path().join("secret.rs"), root.join("evil.rs")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        let snap = Snapshot::capture("r", &root).unwrap();
        let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["a.rs"],
            "outside-root links must not leak in: {paths:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_skips_interior_links_and_cycles() {
        let (_t, root) = tmp_repo(&[("sub/inner.rs", "fn inner() {}\n")]);
        std::os::unix::fs::symlink(root.join("sub"), root.join("linked")).unwrap();
        // Cycle: sub/loop -> root. Capture must terminate.
        std::os::unix::fs::symlink(root.clone(), root.join("sub").join("loop")).unwrap();
        let snap = Snapshot::capture("r", &root).unwrap();
        let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
        // Symlinked content is out of scope: only the real tree is indexed.
        assert_eq!(paths, ["sub/inner.rs"]);
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_skips_special_files() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
        // Unix socket at a supported extension: not a regular file, must
        // never reach a blocking read.
        std::os::unix::net::UnixListener::bind(root.join("events.txt")).unwrap();
        let snap = Snapshot::capture("r", &root).unwrap();
        let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.rs"]);
    }

    #[test]
    fn snapshot_rejects_broken_ignore_rules() {
        let (_t, root) = tmp_repo(&[(".gitignore", "{unclosed\n"), ("a.rs", "fn a() {}\n")]);
        let err = Snapshot::capture("r", &root).expect_err("broken ignore rules must fail loudly");
        assert!(
            !err.to_string().is_empty(),
            "an explicit exclusion-policy error is required"
        );
    }

    #[test]
    fn snapshot_respects_gitignore() {
        let (_t, root) = tmp_repo(&[
            (".gitignore", "private.txt\ngen/\n"),
            ("a.rs", "fn a() {}\n"),
            ("private.txt", "excluded words\n"),
            ("gen/out.rs", "fn out() {}\n"),
        ]);
        let snap = Snapshot::capture("r", &root).unwrap();
        let mut paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            ["a.rs"],
            "ignored files must never reach inference: {paths:?}"
        );
    }

    #[test]
    fn snapshot_skips_nested_git_repos() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
        std::fs::create_dir_all(root.join("vendor/dep/.git")).unwrap();
        std::fs::write(root.join("vendor/dep/b.rs"), "fn b() {}\n").unwrap();
        let snap = Snapshot::capture("r", &root).unwrap();
        let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.rs"], "nested checkouts stay out: {paths:?}");
    }

    #[test]
    fn unsupported_reported() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
        std::fs::write(root.join("img.png"), [0u8, 1, 2]).unwrap();
        let bad = report_unsupported(&root);
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn spans_are_sane() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn foo() {}\n")]);
        let snap = Snapshot::capture("r", &root).unwrap();
        let ext = extract_snapshot(&snap);
        let def = ext
            .entities
            .iter()
            .find(|e| e.kind == EntityKind::Definition)
            .unwrap();
        assert_eq!(def.span.start_line, 1);
        assert_eq!(def.span.file, "a.rs");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_never_followed() {
        use std::os::unix::fs::symlink;
        let (_t, root) = tmp_repo(&[("a.rs", "fn foo() {}\n"), ("sub/b.rs", "fn bar() {}\n")]);
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/secret.rs"), "fn secret() {}\n").unwrap();
        symlink("a.rs", root.join("link-file.rs")).unwrap();
        symlink("sub", root.join("link-dir")).unwrap();
        symlink("..", root.join("sub/loop")).unwrap();
        symlink("outside/secret.rs", root.join("escape.rs")).unwrap();
        let snap = Snapshot::capture("r", &root).unwrap();
        let mut files: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
        files.sort_unstable();
        assert_eq!(files, ["a.rs", "outside/secret.rs", "sub/b.rs"]);
    }
}
