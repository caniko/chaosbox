//! Source verification occurs at capture/import; only bounded capsules travel.
use std::collections::{BTreeMap, BTreeSet};
use chaosbox_core::sha256_hex;
use crate::intelligence::{Bundle, extract_complete_window};
use super::Publication;

/// Reconstruct every retained candidate against the original normalized source.
/// Unreferenced rejected proposals stay in source custody, outside replicated knowledge.
pub fn publication_from_sources(
    mut bundle: Bundle,
    mut source: impl FnMut(&str) -> Result<String, String>,
) -> Result<Publication, String> {
    bundle.validate()?;
    let needed: BTreeSet<_> = bundle
        .records
        .iter()
        .flat_map(|r| r.assessments.clone())
        .collect();
    bundle.assessments.retain(|a| needed.contains(&a.id));
    let mut sources = BTreeMap::new();
    let mut candidates = BTreeMap::new();
    for receipt in &bundle.assessments {
        if candidates.contains_key(&receipt.candidate_id) {
            continue;
        }
        let evidence = bundle
            .records
            .iter()
            .filter(|r| r.repositories == receipt.repositories)
            .flat_map(|r| &r.evidence)
            .find(|e| {
                serde_json::to_string(e).is_ok_and(|s| sha256_hex(&[&s]) == receipt.evidence_digest)
            })
            .ok_or("receipt has no retained source occurrence")?;
        if !sources.contains_key(&evidence.snapshot) {
            let text = source(&evidence.snapshot)?;
            if text.len() > 32 * 1024 * 1024 || sha256_hex(&[&text]) != evidence.snapshot {
                return Err("original source snapshot digest mismatch".into());
            }
            sources.insert(evidence.snapshot.clone(), text);
        }
        let text = &sources[&evidence.snapshot];
        let mut skip = 0;
        loop {
            let catalog = extract_complete_window(
                text,
                &evidence.source,
                &evidence.session,
                &bundle.scope,
                &receipt.repositories,
                skip,
                200,
            )?;
            if let Some(c) = catalog
                .candidates
                .iter()
                .find(|c| c.id == receipt.candidate_id)
            {
                candidates.insert(c.id.clone(), c.clone());
                break;
            }
            if !catalog.has_more {
                return Err("receipt candidate does not resolve in original source".into());
            }
            skip += catalog.candidates.len();
        }
    }
    let publication = Publication {
        bundle,
        candidates: candidates.into_values().collect(),
    };
    publication.validate(&publication.bundle.scope)?;
    Ok(publication)
}
