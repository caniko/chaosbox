//! Shared relation-local reuse resolver (issue #12).
//!
//! One choke point for `uncached_decisions`, `Pipeline::decide`, and
//! `decide_cached`: same reuse inputs, same validation, same Failed-retry
//! rule. Preflight and execution agree by construction — they call this.

use super::{
    BTreeMap, BTreeSet, Candidate, DecisionOutcome, Entity, EvidenceClass, Materialization,
    PipelineError, ReuseContext, cache_key, materialize_raw, questions_for, reuse_input_for,
};
use chaosbox_core::InferenceRecord;

/// Validated hit: the stored inference plus its rematerialization under
/// current thresholds.
#[derive(Clone, Debug)]
pub struct ReuseHit {
    /// Stored reusable inference (authoritative winner).
    pub inference: InferenceRecord,
    /// Rematerialized outcome under current thresholds.
    pub outcome: DecisionOutcome,
    /// Rematerialized evidence class.
    pub evidence_class: EvidenceClass,
    /// Rematerialized confidence, if the answer type carries one.
    pub confidence: Option<f64>,
    /// Rematerialized probability, if applicable.
    pub probability: Option<f64>,
}

/// One resolver verdict: the relation-local key, the current questions, and
/// either a validated hit or a miss (re-ask or skip). `Err` is fail-closed
/// (bindings, missing hashes, multi-question, model or key mismatch,
/// invalid raw); `Ok` with `hit: None` is a miss (no inference, Failed
/// retry).
#[derive(Clone, Debug)]
pub struct ReuseAttempt {
    /// Relation-local reuse key (`jev-reuse:...`).
    pub reuse_key: String,
    /// Current questions (single entry; multi-question is `Err`).
    pub questions: BTreeMap<String, chaosbox_jev::Question>,
    /// Single question id (`rel_<candidate-id>`).
    pub qid: String,
    /// Audit cache key: legacy repo-wide key plus the materialization
    /// digest, so threshold changes replace stored decisions/evidence.
    pub audit_cache_key: String,
    /// Materialization digest (threshold identity).
    pub mat_digest: String,
    /// Validated hit, if reusable.
    pub hit: Option<ReuseHit>,
}

/// Valid options for the single pipeline question: accept/reject/none.
/// Shared by fresh validation and reuse validation so both agree.
#[must_use]
pub fn valid_options_for(qid: &str) -> BTreeMap<String, BTreeSet<String>> {
    BTreeMap::from([(
        qid.to_owned(),
        BTreeSet::from(["accept".into(), "reject".into(), "none".into()]),
    )])
}

/// Shared record validation: the single choke point for cached hits AND
/// freshly persisted winners (issue #12 review).
///
/// Checks, in order:
/// * stored `reuse_key` equals the expected relation-local key;
/// * stored `model_requested` equals the requested model;
/// * stored `model_returned` is nonempty;
/// * stored raw converts to a typed answer passing the same
///   `validate_response` fresh inference passes (type match, finite/range,
///   distribution sum, option membership against the *current* questions).
///
/// Returns the typed answer for rematerialization. `Err` is fail-closed
/// corruption (never silent reuse, never spend-every-run misses against
/// first-write-wins poison).
pub fn validate_stored_inference(
    inf: &InferenceRecord,
    expected_reuse_key: &str,
    ctx_model: &str,
    questions: &BTreeMap<String, chaosbox_jev::Question>,
    qid: &str,
) -> Result<chaosbox_jev::Answer, PipelineError> {
    if inf.reuse_key != expected_reuse_key {
        return Err(PipelineError::Validation(format!(
            "inference reuse-key mismatch: expected {expected_reuse_key}, got {}",
            inf.reuse_key
        )));
    }
    if inf.model_requested != ctx_model {
        return Err(PipelineError::Validation(format!(
            "inference model_requested mismatch for {expected_reuse_key}"
        )));
    }
    if inf.model_returned.is_empty() {
        return Err(PipelineError::Validation(format!(
            "inference missing model_returned for {expected_reuse_key}"
        )));
    }
    let answer = chaosbox_jev::answer_from_raw(&inf.raw);
    let resp = chaosbox_jev::SystemOneResponse {
        model: inf.model_returned.clone(),
        answers: BTreeMap::from([(qid.to_owned(), answer.clone())]),
        usage: chaosbox_jev::Usage {
            input_tokens: 0,
            output_tokens: 0,
        },
    };
    chaosbox_jev::validate_response(&resp, questions, &valid_options_for(qid)).map_err(|e| {
        PipelineError::Validation(format!(
            "stored inference fails current validation for {expected_reuse_key}: {e}"
        ))
    })?;
    Ok(answer)
}

/// Resolve one candidate against the store: compute its relation-local key,
/// look up the inference, validate it against the *current* questions, model
/// provenance, and Failed-retry rule, and rematerialize under current
/// thresholds.
///
/// Validation performed on every hit:
/// * bindings (repo, snapshot, candidate refs) via `reuse_input_for`;
/// * exactly one question (multi-question is fail-closed `Err`);
/// * stored `model_requested` equals the requested model; stored
///   `model_returned` is nonempty — mismatches are corruption (`Err`);
/// * stored raw converts to a typed answer that passes the same
///   `validate_response` fresh inference passes — failures are corruption
///   (`Err`), not spend-every-run misses against first-write-wins poison;
/// * no `Failed` decision with the same reuse key blocks reuse (retries
///   always re-ask — `Ok(None)` miss).
#[allow(clippy::too_many_arguments)]
pub async fn resolve_reuse<S: chaosbox_store::Store>(
    cand: &Candidate,
    from: &Entity,
    to: &Entity,
    snapshot_id: &str,
    ctx: &ReuseContext<'_>,
    mat: &Materialization,
    catalog_digest: &str,
    store: &S,
) -> Result<ReuseAttempt, PipelineError> {
    let questions = questions_for(cand, from, to);
    if questions.len() != 1 {
        return Err(PipelineError::Validation(
            "expected exactly one question per candidate".into(),
        ));
    }
    let qid = questions.keys().next().expect("checked above").clone();
    let old_key = cache_key(
        &from.snapshot,
        catalog_digest,
        &questions,
        ctx.model,
        ctx.rubric_version,
        ctx.policy_digest,
    );
    let mat_digest = mat.digest();
    let audit_cache_key = format!("{old_key}:{mat_digest}");
    let input = reuse_input_for(cand, from, to, &questions, ctx, snapshot_id)?;
    let rkey = chaosbox_jev::reuse_key(&input);
    let base = ReuseAttempt {
        reuse_key: rkey.clone(),
        questions: questions.clone(),
        qid: qid.clone(),
        audit_cache_key,
        mat_digest,
        hit: None,
    };
    let Some(inf) = store
        .find_inference(&rkey)
        .await
        .map_err(|e| PipelineError::Store(e.to_string()))?
    else {
        return Ok(base);
    };
    // Shared validation: key equality, model provenance, current-question
    // semantics. Corruption fails closed (never silent reuse, never
    // spend-every-run misses against first-write-wins poison).
    let _answer = validate_stored_inference(&inf, &rkey, ctx.model, &questions, &qid)?;
    // Failed-always-retry: a recorded `Failed` with the same reuse key
    // blocks reuse even though an inference exists (retries re-ask).
    if let Some(d) = store
        .find_decision(&cand.id, &qid)
        .await
        .map_err(|e| PipelineError::Store(e.to_string()))?
    {
        if matches!(d.outcome, DecisionOutcome::Failed(_)) && d.reuse_key == rkey {
            return Ok(base);
        }
    }
    // Fail-closed: a validated raw that will not materialize is corruption,
    // not a miss that spends every run against first-write-wins poison.
    let (outcome, class, conf, prob) = materialize_raw(&inf.raw, mat, &cand.reason)?;
    Ok(ReuseAttempt {
        hit: Some(ReuseHit {
            inference: inf,
            outcome,
            evidence_class: class,
            confidence: conf,
            probability: prob,
        }),
        ..base
    })
}
