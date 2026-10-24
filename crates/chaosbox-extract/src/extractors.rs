//! File-type dispatch, span plumbing, and the per-language extractors.

use std::path::Path;

use chaosbox_core::{Entity, EntityKind, SourceSpan, RelationType, coverage::FileCoverage};
use regex::Regex;
use serde::{Deserialize, Serialize};

pub(crate) fn is_supported(path: &str) -> bool {
    const EXTS: [&str; 14] = [
        "rs", "py", "js", "ts", "jsx", "tsx", "mts", "cts", "mjs", "cjs", "nix", "md", "markdown",
        "txt",
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
    /// Uncertified `(from_id, to_id, kind)` proposals, subject to decisions.
    pub explicit_refs: Vec<(String, String, String)>,
    /// Certified syntax facts, independent of inference candidate caps.
    #[serde(default)]
    pub facts: Vec<StructuralFact>,
    /// Per-file syntax coverage.
    #[serde(default)]
    pub coverage: Vec<FileCoverage>,
    /// Optional context-bound compiler facts and processing accounting.
    #[serde(default)]
    pub compiler: Option<chaosbox_core::compiler::CompilerCoverage>,
}

/// One source-backed syntax observation. Only containment and declarations
/// are certified by the current extraction contract (no name resolution).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StructuralFact {
    /// Source entity id.
    pub from: String,
    /// Target declaration/module occurrence id.
    pub to: String,
    /// Certified relationship kind.
    pub rel_type: RelationType,
    /// Exact source range proving the declaration, or file/module point.
    pub span: SourceSpan,
    /// Verbatim contents of the range.
    pub text: String,
    /// Parser/grammar/contract version.
    pub producer: String,
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

/// Byte cap for [`Entity::text`]: keeps one embedding input inside the
/// embedding server's 512-token batch.
pub(crate) const ENTITY_TEXT_MAX: usize = 1024;

/// The declaration at `start..end` plus its contiguous leading comment,
/// attribute and decorator lines, trimmed and capped at [`ENTITY_TEXT_MAX`].
pub(crate) fn item_text(text: &str, start: usize, end: usize) -> Option<String> {
    let line_start = |byte: usize| text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let mut from = line_start(start);
    while from > 0 {
        let previous = line_start(from - 1);
        let line = text[previous..from - 1].trim_start();
        if !["//", "/*", "*", "#", "@"]
            .iter()
            .any(|p| line.starts_with(p))
        {
            break;
        }
        from = previous;
    }
    cap_text(&text[from..end.max(start)])
}

/// Trim and cap at [`ENTITY_TEXT_MAX`] on a character boundary.
fn cap_text(item: &str) -> Option<String> {
    let item = item.trim();
    let mut cut = item.len().min(ENTITY_TEXT_MAX);
    while !item.is_char_boundary(cut) {
        cut -= 1;
    }
    let item = item[..cut].trim_end();
    (!item.is_empty()).then(|| item.to_owned())
}

/// Extract one file deterministically.
#[must_use]
pub fn extract_file(repo: &str, snapshot: &str, path: &str, text: &str) -> Extraction {
    if let Some(extraction) = crate::syntax::extract(repo, snapshot, path, text) {
        return extraction;
    }
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
        facts: Vec::new(),
        compiler: None,
        coverage: vec![FileCoverage {
            file: path.to_owned(),
            producer: "legacy-regex-v1".into(),
            status: chaosbox_core::coverage::SyntaxStatus::Heuristic,
            facts: 0,
        }],
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
        let mut e = Entity::new(
            EntityKind::Definition,
            repo,
            snapshot,
            path,
            name,
            &qn,
            span_of(text, path, m.start(), m.end()),
        );
        let line_end = text[m.end()..]
            .find('\n')
            .map_or(text.len(), |i| m.end() + i);
        e.text = item_text(text, m.start(), line_end);
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
        let mut e = Entity::new(
            EntityKind::Heading,
            repo,
            snapshot,
            path,
            title.as_str(),
            &format!("{path}#{}", title.as_str()),
            span_of(text, path, title.start(), title.end()),
        );
        // Heading plus its first paragraph; never the next heading.
        let rest = &text[title.end()..];
        let para = title.end() + rest.len() - rest.trim_start().len();
        let para_end = if text[para..].starts_with('#') {
            title.end()
        } else {
            text[para..].find("\n\n").map_or(text.len(), |i| para + i)
        };
        e.text = cap_text(&text[cap.get(0).unwrap().start()..para_end]);
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
