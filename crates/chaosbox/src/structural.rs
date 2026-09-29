//! Materialize certified syntax/compiler observations without model decisions.

use std::collections::{BTreeMap, BTreeSet};

use chaosbox_core::{
    deterministic_id, sha256_hex, Claim, Entity, EntityKind, Evidence, EvidenceClass, GraphBuild,
    Relation, RelationScope, RelationType,
};
use chaosbox_extract::{Extraction, Snapshot};
use chaosbox_store::Store;

use crate::PipelineError;

fn validate_facts(
    repo: &str,
    snapshot: &Snapshot,
    extraction: &Extraction,
    entities: &BTreeMap<String, Entity>,
) -> Result<(), PipelineError> {
    let mut verified_files = BTreeSet::new();
    if let Some(compiler) = &extraction.compiler {
        let facts: Vec<_> = extraction
            .facts
            .iter()
            .filter(|f| f.producer == compiler.context.producer)
            .collect();
        if facts.len() != compiler.relations()
            || facts
                .iter()
                .filter(|f| f.rel_type == RelationType::Defines)
                .count()
                != compiler.definitions
            || facts
                .iter()
                .filter(|f| f.rel_type == RelationType::References)
                .count()
                != compiler.references
            || facts
                .iter()
                .filter(|f| f.rel_type == RelationType::Implements)
                .count()
                != compiler.implementations
        {
            return Err(PipelineError::Validation(
                "compiler coverage disagrees with facts".into(),
            ));
        }
    }
    // Validate the entire batch before writing. A stale or cross-snapshot
    // extraction must not certify facts against different source bytes.
    for fact in &extraction.facts {
        let from = entities.get(&fact.from);
        let to = entities.get(&fact.to);
        let valid_endpoints = from.zip(to).is_some_and(|(from, to)| {
            from.repo == repo
                && to.repo == repo
                && from.snapshot == snapshot.id
                && to.snapshot == snapshot.id
                && valid_fact_endpoints(from, to, fact, extraction)
        });
        let content = snapshot.contents.get(&fact.span.file);
        if verified_files.insert(&fact.span.file) {
            let valid_version = content
                .zip(snapshot.file_version(&fact.span.file))
                .is_some_and(|(text, version)| {
                    version.sha256 == sha256_hex(&[text]) && version.bytes == text.len() as u64
                });
            if !valid_version {
                return Err(PipelineError::Validation(
                    "certified source content identity mismatch".into(),
                ));
            }
        }
        let valid_source = content.is_some_and(|text| {
            text.get(fact.span.byte_start as usize..fact.span.byte_end as usize)
                == Some(fact.text.as_str())
        });
        if repo != snapshot.repo || !valid_endpoints || !valid_source || fact.producer.is_empty() {
            return Err(PipelineError::Validation(
                "invalid certified source observation".into(),
            ));
        }
    }
    Ok(())
}

fn valid_fact_endpoints(
    from: &Entity,
    to: &Entity,
    fact: &chaosbox_extract::StructuralFact,
    extraction: &Extraction,
) -> bool {
    let declaration = from.kind == EntityKind::File
        && from.file == to.file
        && to.span == fact.span
        && matches!(
            (&fact.rel_type, &to.kind),
            (RelationType::Defines, EntityKind::Definition)
                | (RelationType::Contains, EntityKind::Module)
        );
    if from.compiler.is_none() && to.compiler.is_none() {
        return declaration;
    }
    let Some(coverage) = &extraction.compiler else {
        return false;
    };
    if fact.producer != coverage.context.producer {
        return false;
    }
    let Some(target) = &to.compiler else {
        return false;
    };
    if target.context != coverage.context.id || to.kind != EntityKind::Definition {
        return false;
    }
    if declaration {
        return fact.rel_type == RelationType::Defines;
    }
    from.compiler.as_ref().is_some_and(|source| {
        source.context == coverage.context.id
            && from.span == fact.span
            && match fact.rel_type {
                RelationType::References => {
                    source.anchor == target.anchor && from.kind == EntityKind::Symbol
                }
                RelationType::Implements => from.kind == EntityKind::Definition,
                _ => false,
            }
    })
}

pub(crate) async fn publish_facts<S: Store>(
    store: &mut S,
    build: &mut GraphBuild,
    snapshot: &Snapshot,
    extraction: &Extraction,
    entities: &BTreeMap<String, Entity>,
) -> Result<(), PipelineError> {
    validate_facts(&build.repo, snapshot, extraction, entities)?;
    let mut observations = Vec::new();
    for fact in &extraction.facts {
        let mut relation = Relation::new(
            fact.rel_type.clone(),
            &fact.from,
            &fact.to,
            if entities[&fact.from].file == entities[&fact.to].file {
                RelationScope::File
            } else {
                RelationScope::CrossFile
            },
            &build.id,
        );
        let evidence = Evidence {
            id: deterministic_id(
                "ev",
                &[
                    &snapshot.id,
                    &fact.from,
                    &fact.to,
                    &format!("{:?}", fact.rel_type),
                    &fact.producer,
                    &fact.text,
                ],
            ),
            class: EvidenceClass::Extracted,
            supports: true,
            text: fact.text.clone(),
            span: Some(fact.span.clone()),
            snapshot: snapshot.id.clone(),
            source_file_version: fact.span.file.clone(),
            producer: Some(fact.producer.clone()),
        };
        relation.evidence_ids.push(evidence.id.clone());
        build
            .add_edge(relation.clone())
            .map_err(|e| PipelineError::Validation(e.to_string()))?;
        observations.push((relation, evidence));
    }
    store
        .ensure_snapshot_files(&snapshot.id, &snapshot.repo, &snapshot.snapshot_files())
        .await
        .map_err(|e| PipelineError::Store(e.to_string()))?;
    for (relation, evidence) in observations {
        let claim = Claim {
            id: deterministic_id("claim", &[&relation.id]),
            relation_id: relation.id,
            supporting: vec![evidence.id.clone()],
            contradicting: Vec::new(),
            accepted: true,
        };
        store
            .put_evidence(evidence)
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
        store
            .put_claim(claim)
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
    }
    Ok(())
}
