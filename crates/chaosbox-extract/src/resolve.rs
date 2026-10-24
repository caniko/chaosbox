//! Snapshot-level deterministic name resolution (`unique-name-v1`,
//! `citations-v1`). Results are INFERRED observations citing the use site:
//! a unique lexical match, never compiler resolution. A missing edge never
//! means "nothing calls this".

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::LazyLock,
};

use chaosbox_core::{Entity, EntityKind, RelationType};
use regex::Regex;

use crate::{
    extractors::span_of,
    syntax::{Site, SiteFrom},
    Snapshot, StructuralFact,
};

static CITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(ADR|RFC)[-_ ]?0*(\d{1,5})\b").expect("static regex"));

type Seen = HashSet<(RelationType, String, String)>;

fn seen(facts: &[StructuralFact]) -> Seen {
    facts
        .iter()
        .map(|f| (f.rel_type.clone(), f.from.clone(), f.to.clone()))
        .collect()
}

/// `true` for Rust, `false` for the TypeScript family, `None` otherwise.
fn language(path: &str) -> Option<bool> {
    match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "rs" => Some(true),
        "ts" | "tsx" | "mts" | "cts" => Some(false),
        _ => None,
    }
}

fn unique<'a>(mut it: impl Iterator<Item = &'a Entity>) -> Option<&'a Entity> {
    match (it.next(), it.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// Resolve call/heritage sites to unique same-language definitions: unique
/// in the use site's file, else (unless file-local) unique in the snapshot.
/// `Owner::name` requires `name` to be unique in the (uniquely resolved)
/// owner's file, or else unique among module files named like the owner
/// (`src/owner.rs`, `src/owner/mod.rs`). That keeps `String::new` and
/// `Vec::with_capacity` unresolved.
pub(crate) fn names(sites: &[Site], entities: &[Entity], facts: &mut Vec<StructuralFact>) {
    let mut index: BTreeMap<(bool, &str), Vec<&Entity>> = BTreeMap::new();
    for e in entities {
        if e.kind == EntityKind::Definition && e.compiler.is_none() {
            if let Some(rust) = language(&e.file) {
                index.entry((rust, e.name.as_str())).or_default().push(e);
            }
        }
    }
    let pick = |rust: bool, name: &str, file: &str, local: bool| -> Option<&Entity> {
        let all = index.get(&(rust, name))?;
        let here: Vec<_> = all.iter().filter(|e| e.file == file).collect();
        match here.as_slice() {
            [one] => Some(**one),
            [] if !local && all.len() == 1 => Some(all[0]),
            _ => None,
        }
    };
    let mut done = seen(facts);
    for site in sites {
        let file = site.span.file.as_str();
        let to = match site.qualifier.as_deref() {
            None | Some("crate" | "super") => pick(site.rust, &site.name, file, site.local),
            Some("self" | "Self") => pick(site.rust, &site.name, file, true),
            Some(owner) => index.get(&(site.rust, site.name.as_str())).and_then(|all| {
                pick(site.rust, owner, file, false)
                    .and_then(|owner_def| {
                        unique(all.iter().copied().filter(|e| e.file == owner_def.file))
                    })
                    .or_else(|| {
                        unique(
                            all.iter()
                                .copied()
                                .filter(|e| module_name(&e.file) == Some(owner)),
                        )
                    })
            }),
        };
        let Some(to) = to else {
            continue;
        };
        let from = match &site.from {
            SiteFrom::Entity(id) => id.clone(),
            SiteFrom::Name(name) => match pick(site.rust, name, file, true) {
                Some(e) => e.id.clone(),
                None => continue,
            },
        };
        if from == to.id || !done.insert((site.rel.clone(), from.clone(), to.id.clone())) {
            continue;
        }
        facts.push(StructuralFact {
            from,
            to: to.id.clone(),
            rel_type: site.rel.clone(),
            text: site.name.clone(),
            span: site.span.clone(),
            producer: format!("unique-name-v1/{}", site.producer),
            resolved: true,
        });
    }
}

/// `src/store.rs` and `src/store/mod.rs` name the module `store`.
fn module_name(path: &str) -> Option<&str> {
    let p = Path::new(path);
    match p.file_stem()?.to_str()? {
        "mod" | "lib" | "main" | "index" => p.parent()?.file_name()?.to_str(),
        stem => Some(stem),
    }
}

fn is_markdown(path: &str) -> bool {
    path.rsplit('.').next().is_some_and(|ext| {
        ["md", "markdown", "mdx"]
            .iter()
            .any(|m| m.eq_ignore_ascii_case(ext))
    })
}

/// Link ADR/RFC citations in rationale comments and Markdown to the unique
/// captured document carrying that number. External RFCs stay plain text.
pub(crate) fn citations(snapshot: &Snapshot, entities: &[Entity], facts: &mut Vec<StructuralFact>) {
    let files: Vec<&Entity> = entities
        .iter()
        .filter(|e| e.kind == EntityKind::File)
        .collect();
    let mut done = seen(facts);
    for source in entities {
        let markdown = source.kind == EntityKind::File && is_markdown(&source.file);
        if !markdown && source.kind != EntityKind::Rationale {
            continue;
        }
        let Some(content) = snapshot.contents.get(&source.file) else {
            continue;
        };
        // Rationale names are exact source slices, so offsets map back.
        let (base, text) = if markdown {
            (0, content.as_str())
        } else {
            (source.span.byte_start as usize, source.name.as_str())
        };
        for found in CITE.captures_iter(text) {
            let kind = found[1].to_ascii_lowercase();
            let Ok(number) = found[2].parse::<u32>() else {
                continue;
            };
            let Some(target) =
                unique(files.iter().copied().filter(|f| {
                    f.file != source.file && doc_number(&f.file, &kind) == Some(number)
                }))
            else {
                continue;
            };
            if !done.insert((
                RelationType::References,
                source.id.clone(),
                target.id.clone(),
            )) {
                continue;
            }
            let whole = found.get(0).expect("whole match");
            facts.push(StructuralFact {
                from: source.id.clone(),
                to: target.id.clone(),
                rel_type: RelationType::References,
                text: whole.as_str().to_owned(),
                span: span_of(
                    content,
                    &source.file,
                    base + whole.start(),
                    base + whole.end(),
                ),
                producer: "citations-v1".into(),
                resolved: true,
            });
        }
    }
}

/// Number of an ADR/RFC document: `adr-0012-x.md`, `RFC9110.md`, or
/// `0012-x.md` directly under an `adr(s)`/`decisions` or `rfc(s)` directory.
fn doc_number(path: &str, kind: &str) -> Option<u32> {
    let p = Path::new(path);
    let stem = p.file_stem()?.to_str()?.to_ascii_lowercase();
    let dir = p
        .parent()
        .and_then(Path::file_name)
        .and_then(|d| d.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let in_dir = if kind == "adr" {
        matches!(dir.as_str(), "adr" | "adrs" | "decisions")
    } else {
        matches!(dir.as_str(), "rfc" | "rfcs")
    };
    let rest = match stem.strip_prefix(kind) {
        Some(rest) => rest.trim_start_matches(['-', '_', ' ']),
        None if in_dir => stem.as_str(),
        None => return None,
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let separated = rest[digits..]
        .chars()
        .next()
        .is_none_or(|c| matches!(c, '-' | '_' | ' ' | '.'));
    if digits == 0 || !separated {
        return None;
    }
    rest[..digits].parse().ok()
}
