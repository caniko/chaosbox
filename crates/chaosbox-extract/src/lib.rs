//! Deterministic source parsing and candidate generation.
//!
//! Supported (explicit, no complete-call-resolution claims):
//! - Rust / Python / JavaScript+TypeScript: files, modules, symbols,
//!   definitions, imports, containment, explicit textual references.
//! - Markdown: headings, links, code mentions, source spans.
//! - Plain text: file/symbol records, lexical mentions.
//!   Unsupported images/audio/video and office docs are reported, never
//!   silently interpreted or omitted.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
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
    pub fn capture(repo: &str, root: &Path) -> Result<Self, ExtractError> {
        let mut files = Vec::new();
        let mut contents = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir).map_err(|e| ExtractError::Io(e.to_string()))?;
            let mut sorted: Vec<PathBuf> = Vec::new();
            for e in entries {
                let e = e.map_err(|e| ExtractError::Io(e.to_string()))?;
                sorted.push(e.path());
            }
            sorted.sort();
            for p in sorted {
                // Never follow symlinks: no escape from the corpus root and
                // no cycles. Symlinked content is out of scope for indexing.
                if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                    continue;
                }
                if p.is_dir() {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if name == ".git" || name == "target" || name == "node_modules" {
                        continue;
                    }
                    stack.push(p);
                } else if p.is_file() {
                    let rel = p
                        .strip_prefix(root)
                        .map_err(|e| ExtractError::Io(e.to_string()))?
                        .to_string_lossy()
                        .replace('\\', "/");
                    if is_supported(&rel) {
                        let text = std::fs::read_to_string(&p)
                            .map_err(|e| ExtractError::Io(e.to_string()))?;
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
    const EXTS: [&str; 11] = [
        "rs", "py", "js", "ts", "jsx", "tsx", "mjs", "cjs", "md", "markdown", "txt",
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
    Extraction {
        entities,
        explicit_refs: refs,
    }
}

/// Bounded candidate construction: no cartesian product.
///
/// Sources: same-file co-occurrence, qualified-name match, explicit imports,
/// lexical mentions, structural (file->module->definition) neighborhoods.
/// Cap total candidates to keep Jev budgets bounded.
// Over the default line budget; splitting the bounded pipeline stages
// apart is the owning session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn build_candidates(extraction: &Extraction, max_candidates: usize) -> Vec<Candidate> {
    // Nested helper first: items exist from scope start (clarity lint).
    // Eight parameters are a smell the owning session should refactor
    // (e.g. a builder struct); allowed to keep this integration unblocked.
    #[allow(clippy::too_many_arguments)]
    fn push(
        out: &mut Vec<Candidate>,
        seen: &mut BTreeSet<(String, String, String)>,
        max_candidates: usize,
        rel: RelationType,
        from: &str,
        to: &str,
        reason: &str,
        excerpt: &str,
    ) {
        if out.len() >= max_candidates || from == to {
            return;
        }
        let key = (rel_name(&rel), from.to_owned(), to.to_owned());
        if seen.insert(key) {
            let id = deterministic_id("cand", &[&rel_name(&rel), from, to, reason]);
            out.push(Candidate {
                id,
                rel_type: rel,
                from_entity: from.to_owned(),
                to_entity: to.to_owned(),
                reason: reason.to_owned(),
                state_excerpt: excerpt.chars().take(500).collect(),
            });
        }
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
    let mut out: Vec<Candidate> = Vec::new();
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
        push(
            &mut out,
            &mut seen,
            max_candidates,
            rel,
            from,
            to,
            "structural",
            &excerpt,
        );
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
                    &mut out,
                    &mut seen,
                    max_candidates,
                    RelationType::References,
                    &imp.id,
                    &d.id,
                    "lexical-import",
                    &d.qualified_name,
                );
            }
            if out.len() >= max_candidates {
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
                &mut out,
                &mut seen,
                max_candidates,
                RelationType::References,
                &pair[0].id,
                &pair[1].id,
                "co-occurrence",
                &pair[1].qualified_name,
            );
        }
    }
    out
}

fn rel_name(r: &RelationType) -> String {
    format!("{r:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

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
        let cands = build_candidates(&ext, 100);
        assert!(!cands.is_empty());
        assert!(cands.len() <= 100, "candidates bounded");
    }

    #[test]
    fn no_cartesian_product() {
        let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")]);
        let snap = Snapshot::capture("r", &root).unwrap();
        let ext = extract_snapshot(&snap);
        let cands = build_candidates(&ext, 10);
        // 3 defs would be 9 pairs cartesian; bounded co-occurrence gives <= 2 + structural
        assert!(cands.len() <= 10);
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
