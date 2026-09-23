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
pub mod intelligence;
pub mod sessions;
mod lifecycle;
mod materialization;
mod pipeline;
mod reader;
mod responder;
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
