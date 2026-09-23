//! Chaosbox orchestration: snapshot -> extract -> candidates -> Jev decisions
//! -> validated evidence/claims -> policy build -> atomic publication ->
//! read-only consumers. One shared query implementation serves CLI and MCP.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chaosbox_core::{
    catalog_digest, check_confidence, check_probability, deterministic_id, Candidate, Claim,
    Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild, Relation,
    RelationScope,
};
use chaosbox_extract::{build_candidates, extract_snapshot, CandidateCatalog, Extraction, Snapshot};
use chaosbox_store::MemoryStore;
use chaosbox_jev::{
    cache_key, Answer, ChoiceAnswer, JevClient, NoulAnswer, Question, ScoreAnswer,
    SystemOneResponse,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod history;
pub mod intelligence;
mod lifecycle;
mod materialization;
mod pipeline;
mod reader;
mod responder;
pub mod sessions;
mod view;

pub use lifecycle::LifecycleReport;
pub use materialization::{Materialization, questions_for};
pub use pipeline::{Pipeline, chain_publication, summarize_outcomes, uncached_decisions};
pub use reader::{
    EXPORT_EDGE_CAP, EXPORT_NODE_CAP, GraphReader, all_relation_types, validate_rel_filter,
};
pub use responder::{FixtureResponder, LiveResponder, Responder};
pub use view::{export_json, explain_entity, search};

/// Pipeline failures across extraction, inference, validation, storage, and consumers.
#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("extract: {0}")]
    /// Deterministic extraction or snapshot capture failed.
    Extract(String),
    #[error("jev: {0}")]
    /// The Jev decision step failed (transport, budget, or protocol).
    Jev(String),
    #[error("validation: {0}")]
    /// A validated evidence/claim/decision check failed.
    Validation(String),
    #[error("store: {0}")]
    /// Persistence or publication failed.
    Store(String),
    #[error("consumer: {0}")]
    /// A read-only consumer query failed.
    Consumer(String),
}

/// Assemble evidence deterministically from a decision: ids and texts are
/// pure functions of (decision, entity), so cache reuse rebuilds
/// byte-identical rows and puts stay idempotent. Single choke point —
/// success, failure, and skip paths must all use this.
fn assemble_evidence(
    decision: &Decision,
    supports: bool,
    text: String,
    span: Option<chaosbox_core::SourceSpan>,
    snapshot: &str,
    file: &str,
    suffix: &str,
) -> Evidence {
    Evidence {
        id: deterministic_id("ev", &[&decision.id, suffix]),
        class: decision.evidence_class,
        supports,
        text,
        span,
        snapshot: snapshot.to_owned(),
        source_file_version: file.to_owned(),
    }
}

/// Cache-only decisions for an entities-only refresh: reuse every decision
/// live [`Pipeline::decide`] would have reused and skip the rest, instead
/// of asking a responder or returning nothing at all.
///
/// A `--no-decisions` run therefore republishes the relations an earlier
/// decisions run already paid for rather than swinging the active pointer
/// to a node-only build, while still never inventing an inference the
/// operator did not authorize: an uncached candidate contributes nothing
/// here and is simply asked again by the next decisions run.
///
/// The cache test mirrors [`Pipeline::decide`] verbatim (same catalog
/// digest, question set, model and rubric inputs); if decide's reuse rule
/// changes, this function must change with it. The exit-4 `coverage:` gate
/// in `main.rs` rests on exactly this test holding.
pub async fn decide_cached<S: chaosbox_store::Store>(
    candidates: &[Candidate],
    entities: &BTreeMap<String, Entity>,
    model_requested: &str,
    mat: &Materialization,
    store: &mut S,
) -> Result<Vec<(Candidate, Decision, Evidence)>, PipelineError> {
    mat.validate()?;
    let catalog = catalog_digest(candidates);
    let mut out = Vec::new();
    for cand in candidates {
        let from = entities
            .get(&cand.from_entity)
            .ok_or_else(|| PipelineError::Validation("missing from".into()))?;
        let to = entities
            .get(&cand.to_entity)
            .ok_or_else(|| PipelineError::Validation("missing to".into()))?;
        let questions = questions_for(cand, from, to);
        let qid = format!("rel_{}", cand.id);
        let key = cache_key(
            &from.snapshot,
            &catalog,
            &questions,
            model_requested,
            &mat.rubric_version,
        );
        let Some(stored) = store
            .find_decision(&cand.id, &qid)
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?
        else {
            continue;
        };
        if stored.cache_key != key || matches!(stored.outcome, DecisionOutcome::Failed(_)) {
            continue;
        }
        let supports = stored.outcome == DecisionOutcome::Accepted;
        let text = format!(
            "[{}] {} -> {} ({:?})",
            cand.reason, from.qualified_name, to.qualified_name, cand.rel_type
        );
        let ev = assemble_evidence(
            &stored,
            supports,
            text,
            Some(from.span.clone()),
            &from.snapshot,
            &from.file,
            "support",
        );
        store
            .put_evidence(ev.clone())
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
        out.push((cand.clone(), stored, ev));
    }
    Ok(out)
}

/// Whether a cache-only (`--no-decisions`) refresh may swing the active
/// pointer.
///
/// Cache identity is bound to the repository snapshot and the whole catalog,
/// so an ordinary source edit leaves a capture-only run with nothing to reuse:
/// publishing that under-covered graph would replace a relation-bearing build
/// with a node-only one and silently drop the relations consumers are querying
/// today. Therefore:
///
/// * no active build at all always publishes: a first capture-only publish
///   protects nothing and enables everything, and without it no build could
///   ever exist (snapshot mode is what bootstraps a repository before
///   anyone consents to infer anything);
/// * a repository that publishes no relations yet always refreshes, so entity
///   indexing keeps working before anyone consents to infer anything;
/// * a repository whose relations are live only publishes when every current
///   candidate still has a reusable decision (including the degenerate
///   "assessed nothing" case, which is not coverage either);
/// * when the active build cannot be read at all, leave it alone — Chaosbox
///   never replaces a build it cannot account for.
///
/// The refused run reports its pending work instead of publishing; the
/// decisions run is what clears it.
#[must_use]
pub fn capture_only_publishable(
    active_publishes_relations: Result<Option<bool>, ()>,
    candidates: usize,
    reusable: usize,
) -> bool {
    match active_publishes_relations {
        Ok(None | Some(false)) => true,
        Ok(Some(true)) => candidates > 0 && reusable >= candidates,
        Err(()) => false,
    }
}

/// Claim evidence helper used by tests: removing one source keeps others.
#[must_use]
pub fn claim_survives_source_removal(claim: &Claim, removed_evidence: &str) -> bool {
    let remaining_support: Vec<_> = claim
        .supporting
        .iter()
        .filter(|e| *e != removed_evidence)
        .collect();
    let remaining_contra: Vec<_> = claim
        .contradicting
        .iter()
        .filter(|e| *e != removed_evidence)
        .collect();
    !remaining_support.is_empty() || !remaining_contra.is_empty() || claim.supporting.is_empty()
}

#[cfg(test)]
mod tests;
