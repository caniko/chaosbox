//! Bounded candidate construction: selection plus truthful accounting.

use std::collections::{BTreeMap, BTreeSet};

use chaosbox_core::{Candidate, Entity, EntityKind, RelationType, deterministic_id};
use serde::{Deserialize, Serialize};

use crate::Extraction;

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
