//! Chaosbox orchestration: snapshot -> extract -> candidates -> Jev decisions
//! -> validated evidence/claims -> policy build -> atomic publication ->
//! read-only consumers. One shared query implementation serves CLI and MCP.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chaosbox_core::{
    catalog_digest, check_confidence, check_probability, deterministic_id, evidence_class_name,
    Candidate, Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild,
    InferenceRecord, RawAnswer, Relation, RelationScope,
};
use chaosbox_extract::{build_candidates, extract_snapshot, CandidateCatalog, Extraction, Snapshot};
use chaosbox_store::MemoryStore;
use chaosbox_jev::{
    cache_key, Answer, ChoiceAnswer, JevClient, NoulAnswer, Question, ScoreAnswer,
    SystemOneResponse,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod compiler;
pub mod history;
pub mod intelligence;
mod lifecycle;
mod materialization;
mod pipeline;
mod reader;
mod responder;
pub(crate) mod reuse;
pub mod sessions;
mod structural;
mod view;
pub mod workspace;

pub use lifecycle::LifecycleReport;
pub use materialization::{
    Materialization, ReuseContext, file_hashes_for, materialize_raw, questions_for, reuse_input_for,
};
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
///
/// Identity is content-addressed: decision id + suffix + supports + class +
/// snapshot + file + text. Two materializations with different outcomes or
/// bindings never share an evidence id, so TypeDB/Memory first-write-wins
/// can never retain a stale row under a colliding id.
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
        id: deterministic_id(
            "ev",
            &[
                &decision.id,
                suffix,
                if supports { "1" } else { "0" },
                &evidence_class_name(decision.evidence_class),
                snapshot,
                file,
                &text,
            ],
        ),
        class: decision.evidence_class,
        supports,
        text,
        span,
        snapshot: snapshot.to_owned(),
        source_file_version: file.to_owned(),
        producer: None,
    }
}

/// Materialized decision identity: candidate + question + requested model +
/// relation-local reuse key + materialization digest.
///
/// Including the reuse key binds the decision to its authoritative
/// inference inputs (endpoints, excerpt, questions, model, rubric, policy):
/// a policy or endpoint change mints a new decision (hence new evidence)
/// instead of colliding with a prior materialization under the same id.
/// Including the materialization digest versions threshold-derived outcomes
/// so rematerialization replaces stale rows.
fn decision_id_for(
    candidate_id: &str,
    question_id_for_id: &str,
    model_requested: &str,
    reuse_key: &str,
    mat_digest: &str,
) -> String {
    deterministic_id(
        "dec",
        &[
            candidate_id,
            question_id_for_id,
            model_requested,
            reuse_key,
            mat_digest,
        ],
    )
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
/// Shares [`crate::reuse::resolve_reuse`] verbatim with
/// [`Pipeline::decide`] and [`crate::uncached_decisions`]. The exit-4
/// `coverage:` gate in `main.rs` rests on exactly this test holding.
pub async fn decide_cached<S: chaosbox_store::Store>(
    candidates: &[Candidate],
    entities: &BTreeMap<String, Entity>,
    snapshot: &Snapshot,
    model_requested: &str,
    mat: &Materialization,
    policy: &chaosbox_core::EffectivePolicy,
    store: &mut S,
) -> Result<Vec<(Candidate, Decision, Evidence)>, PipelineError> {
    mat.validate()?;
    let catalog = catalog_digest(candidates);
    let policy_digest = policy.digest();
    let file_hashes = file_hashes_for(snapshot);
    let ctx = ReuseContext {
        repo: &snapshot.repo,
        file_hashes: &file_hashes,
        model: model_requested,
        rubric_version: &mat.rubric_version,
        policy_digest: &policy_digest,
    };
    let mut out = Vec::new();
    for cand in candidates {
        let from = entities
            .get(&cand.from_entity)
            .ok_or_else(|| PipelineError::Validation("missing from".into()))?;
        let to = entities
            .get(&cand.to_entity)
            .ok_or_else(|| PipelineError::Validation("missing to".into()))?;
        let attempt =
            crate::reuse::resolve_reuse(cand, from, to, &snapshot.id, &ctx, mat, &catalog, &*store)
                .await?;
        let Some(hit) = attempt.hit else {
            continue;
        };
        let decision = Decision {
            id: decision_id_for(
                &cand.id,
                &attempt.qid,
                model_requested,
                &attempt.reuse_key,
                &attempt.mat_digest,
            ),
            candidate_id: cand.id.clone(),
            question_id: attempt.qid.clone(),
            outcome: hit.outcome.clone(),
            evidence_class: hit.evidence_class,
            model_requested: model_requested.to_owned(),
            model_returned: hit.inference.model_returned.clone(),
            confidence: hit.confidence,
            probability: hit.probability,
            cache_key: attempt.audit_cache_key.clone(),
            reuse_key: attempt.reuse_key.clone(),
            raw_answer: Some(hit.inference.raw.clone()),
        };
        let supports = hit.outcome == DecisionOutcome::Accepted;
        let text = format!(
            "[{}] {} -> {} ({:?})",
            cand.reason, from.qualified_name, to.qualified_name, cand.rel_type
        );
        let ev = assemble_evidence(
            &decision,
            supports,
            text,
            Some(from.span.clone()),
            &snapshot.id,
            &from.file,
            "support",
        );
        store
            .put_evidence(ev.clone())
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
        store
            .put_decision(decision.clone())
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
        out.push((cand.clone(), decision, ev));
    }
    Ok(out)
}

/// Whether a cache-only (`--no-decisions`) refresh may swing the active
/// pointer.
///
/// The input flag denotes decision-backed relationships, or any relationships
/// in a legacy build whose provenance is unknown. Certified syntax-only builds
/// can always refresh; relation-local reuse protects paid semantic work:
///
/// * no active build at all always publishes: a first capture-only publish
///   protects nothing and enables everything, and without it no build could
///   ever exist (snapshot mode is what bootstraps a repository before
///   anyone consents to infer anything);
/// * a repository with only certified facts (or no relations) always refreshes;
/// * a repository with decision-backed/legacy relations publishes when every current
///   candidate still has a reusable decision (including the degenerate
///   "assessed nothing" case, which is not coverage either);
/// * when the active build cannot be read at all, leave it alone — Chaosbox
///   never replaces a build it cannot account for.
///
/// The refused run reports its pending work instead of publishing; the
/// decisions run is what clears it.
///
/// The error type is `String` so callers keep their diagnostics: any
/// `Err` means "cannot tell" and must fail, never publish.
#[must_use]
pub fn capture_only_publishable(
    active_publishes_relations: &Result<Option<bool>, String>,
    candidates: usize,
    reusable: usize,
) -> bool {
    match active_publishes_relations {
        Ok(None | Some(false)) => true,
        Ok(Some(true)) => candidates > 0 && reusable >= candidates,
        Err(_) => false,
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
