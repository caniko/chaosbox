//! Materialization identity (rubric and acceptance thresholds) plus the Jev questions a candidate becomes.

use super::{
    Serialize, Deserialize, deterministic_id, PipelineError, Candidate, DecisionOutcome, Entity,
    EvidenceClass, RawAnswer, Snapshot, BTreeMap, Question, check_confidence, check_probability,
};

/// Rubric + acceptance policy. Thresholds are materialization identity:
/// changing them reuses raw decisions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Materialization {
    /// Rubric version gating question semantics; part of cache identity.
    pub rubric_version: String,
    /// Noul acceptance cutoff; also in the materialization identity.
    pub accept_noul: f64,
    /// Choice/Score confidence acceptance cutoff; also in the identity.
    pub accept_confidence: f64,
    /// Score acceptance cutoff; also in the identity.
    pub accept_score: f64,
    /// Confidence floor: Choice/Score answers below it become `Abstained`
    /// (recorded, never retried as failures). Also in the identity.
    /// Must not exceed `accept_confidence` (checked by [`Materialization::validate`]).
    pub abstain_confidence: f64,
}

impl Default for Materialization {
    fn default() -> Self {
        Self {
            rubric_version: "rubric-v1".into(),
            accept_noul: 0.7,
            accept_confidence: 0.6,
            accept_score: 1.0,
            abstain_confidence: 0.4,
        }
    }
}

impl Materialization {
    /// Materialization identity: every threshold plus build inputs, so
    /// threshold changes reuse valid raw decisions instead of re-asking Jev.
    #[must_use]
    pub fn identity(&self, build_inputs: &str) -> String {
        deterministic_id(
            "mat",
            &[
                &self.rubric_version,
                &self.accept_noul.to_string(),
                &self.accept_confidence.to_string(),
                &self.accept_score.to_string(),
                &self.abstain_confidence.to_string(),
                build_inputs,
            ],
        )
    }

    /// Check threshold coherence: the abstain floor must not exceed the
    /// accept floor, otherwise the overlap region has no defined outcome.
    /// Precedence elsewhere is abstain-first.
    pub fn validate(&self) -> Result<(), PipelineError> {
        if self.abstain_confidence > self.accept_confidence {
            return Err(PipelineError::Validation(format!(
                "abstain_confidence {} exceeds accept_confidence {}",
                self.abstain_confidence, self.accept_confidence
            )));
        }
        for (name, v) in [
            ("accept_noul", self.accept_noul),
            ("accept_confidence", self.accept_confidence),
            ("accept_score", self.accept_score),
            ("abstain_confidence", self.abstain_confidence),
        ] {
            if !v.is_finite() {
                return Err(PipelineError::Validation(format!("{name} non-finite")));
            }
        }
        Ok(())
    }

    /// Threshold identity for threshold-derived materializations (issue #12).
    ///
    /// Every threshold plus the rubric version feeds `Decision` identity
    /// (`id` and `cache_key`, hence `Evidence` identity via the decision
    /// id). A threshold change therefore replaces the stored decision and
    /// its evidence instead of leaving a stale row that disagrees with the
    /// returned outcome. Raw inferences stay immutable under their
    /// relation-local reuse key; only the materialization is versioned here.
    #[must_use]
    pub fn digest(&self) -> String {
        deterministic_id(
            "mat",
            &[
                &self.rubric_version,
                &self.accept_noul.to_string(),
                &self.accept_confidence.to_string(),
                &self.accept_score.to_string(),
                &self.abstain_confidence.to_string(),
            ],
        )
    }
}

/// How a candidate becomes Jev questions. Descriptions (not ids) go into
/// state/instructions; ids carry no inference meaning.
#[must_use]
pub fn questions_for(
    candidate: &Candidate,
    from: &Entity,
    to: &Entity,
) -> BTreeMap<String, Question> {
    BTreeMap::from([(
        format!("rel_{}", candidate.id),
        Question::Choice {
            instructions: format!(
                "Given the source evidence, which relation holds from '{}' ({:?}, {}) to '{}' ({:?}, {})? \
                 Answer only from the listed options. Option meanings: accept = evidence supports {:?}; \
                 reject = evidence contradicts; none = no finding (successful negative, do not retry). \
                 Entity descriptions are authoritative; the question id is arbitrary.",
                from.qualified_name,
                from.kind,
                from.file,
                to.qualified_name,
                to.kind,
                to.file,
                candidate.rel_type
            ),
            criteria: BTreeMap::from([
                (
                    "accept".into(),
                    Some(format!(
                        "Source shows {:?} from {} to {}",
                        candidate.rel_type, from.qualified_name, to.qualified_name
                    )),
                ),
                (
                    "reject".into(),
                    Some("Source contradicts the proposed relation".into()),
                ),
                (
                    "none".into(),
                    Some("No finding in source; abstain from the relation".into()),
                ),
            ]),
        },
    )])
}

/// Content hashes for reuse identity, keyed by repository-relative path.
/// Sourced from the snapshot's pinned file versions so reuse is authorized
/// by the bytes the decision reasoned over, not by names alone.
#[must_use]
pub fn file_hashes_for(snapshot: &Snapshot) -> BTreeMap<String, String> {
    snapshot
        .files
        .iter()
        .map(|f| (f.path.clone(), f.sha256.clone()))
        .collect()
}

/// Shared context for building relation-local reuse inputs: everything
/// about the run that is the same for every candidate, so per-candidate
/// calls stay narrow.
pub struct ReuseContext<'a> {
    /// Owning repository name.
    pub repo: &'a str,
    /// Pinned file content hashes by repository-relative path.
    pub file_hashes: &'a BTreeMap<String, String>,
    /// Model identity requested.
    pub model: &'a str,
    /// Rubric version gating question semantics.
    pub rubric_version: &'a str,
    /// Effective-policy digest (scope, privacy, inference).
    pub policy_digest: &'a str,
}

/// Build the versioned relation-local reuse input for one candidate
/// (issue #12).
///
/// Snapshot/entity/candidate ids and question map keys never enter: only
/// the repo, canonical relation/reason, content-pinned endpoints
/// (file + qualified name + kind + file hash), excerpt, canonical question
/// semantics, model, rubric, and policy digest. A missing file hash fails
/// closed — reuse must never be authorized by an unknown byte identity.
///
/// Binding checks (Slice 2B): both endpoints must belong to `ctx.repo` and
/// to the snapshot the file hashes came from; otherwise reuse could mix
/// rows across repositories or snapshots.
pub fn reuse_input_for(
    candidate: &Candidate,
    from: &Entity,
    to: &Entity,
    questions: &BTreeMap<String, Question>,
    ctx: &ReuseContext<'_>,
    snapshot_id: &str,
) -> Result<chaosbox_jev::ReuseInput, PipelineError> {
    if from.repo != ctx.repo || to.repo != ctx.repo {
        return Err(PipelineError::Validation(format!(
            "endpoint repo mismatch: expected {0}, got {1}/{2}",
            ctx.repo, from.repo, to.repo
        )));
    }
    if from.snapshot != snapshot_id || to.snapshot != snapshot_id {
        return Err(PipelineError::Validation(format!(
            "endpoint snapshot mismatch: expected {snapshot_id}, got {}/{}",
            from.snapshot, to.snapshot
        )));
    }
    if candidate.from_entity != from.id || candidate.to_entity != to.id {
        return Err(PipelineError::Validation(
            "candidate does not reference the given endpoints".into(),
        ));
    }
    let from_hash = ctx
        .file_hashes
        .get(&from.file)
        .ok_or_else(|| PipelineError::Validation(format!("missing file hash for {}", from.file)))?;
    let to_hash = ctx
        .file_hashes
        .get(&to.file)
        .ok_or_else(|| PipelineError::Validation(format!("missing file hash for {}", to.file)))?;
    Ok(chaosbox_jev::ReuseInput {
        version: chaosbox_jev::REUSE_VERSION.to_owned(),
        repo: ctx.repo.to_owned(),
        rel_type: chaosbox_core::relation_type_name(&candidate.rel_type),
        reason: candidate.reason.clone(),
        from: chaosbox_jev::ReuseEndpoint {
            file: from.file.clone(),
            qualified: from.qualified_name.clone(),
            kind: chaosbox_core::entity_kind_name(&from.kind),
            file_hash: from_hash.clone(),
        },
        to: chaosbox_jev::ReuseEndpoint {
            file: to.file.clone(),
            qualified: to.qualified_name.clone(),
            kind: chaosbox_core::entity_kind_name(&to.kind),
            file_hash: to_hash.clone(),
        },
        excerpt: candidate.state_excerpt.clone(),
        canonical_questions: chaosbox_jev::canonical_questions(questions),
        model: ctx.model.to_owned(),
        rubric_version: ctx.rubric_version.to_owned(),
        policy_digest: ctx.policy_digest.to_owned(),
    })
}

/// Single choke point turning a validated raw answer into a thresholded
/// outcome (issue #12, Slice 2A).
///
/// Fresh inference and cross-snapshot reuse both go through here, so a
/// threshold change rematerializes the same raw answer instead of re-asking.
/// Abstain-first precedence matches the previous `decide` behavior:
/// below-floor confidence abstains regardless of the selected option or
/// score. `structural` reasons upgrade `Inferred` to `Extracted`; model
/// confidence alone never upgrades.
pub fn materialize_raw(
    raw: &RawAnswer,
    mat: &Materialization,
    reason: &str,
) -> Result<(DecisionOutcome, EvidenceClass, Option<f64>, Option<f64>), PipelineError> {
    let (outcome, class, conf, prob) = match raw {
        RawAnswer::Noul { noul } => {
            check_probability(*noul).map_err(|e| PipelineError::Validation(e.to_string()))?;
            if *noul >= mat.accept_noul {
                (
                    DecisionOutcome::Accepted,
                    EvidenceClass::Inferred,
                    None,
                    Some(*noul),
                )
            } else {
                (
                    DecisionOutcome::Negative,
                    EvidenceClass::Ambiguous,
                    None,
                    Some(*noul),
                )
            }
        }
        RawAnswer::Choice {
            choice,
            probabilities,
            confidence,
        } => {
            check_confidence(*confidence).map_err(|e| PipelineError::Validation(e.to_string()))?;
            for p in probabilities.values() {
                check_probability(*p).map_err(|e| PipelineError::Validation(e.to_string()))?;
            }
            if *confidence < mat.abstain_confidence {
                (
                    DecisionOutcome::Abstained,
                    EvidenceClass::Ambiguous,
                    Some(*confidence),
                    None,
                )
            } else {
                match choice.as_str() {
                    "accept" => (
                        DecisionOutcome::Accepted,
                        EvidenceClass::Inferred,
                        Some(*confidence),
                        probabilities.get("accept").copied(),
                    ),
                    "reject" => (
                        DecisionOutcome::Rejected,
                        EvidenceClass::Ambiguous,
                        Some(*confidence),
                        probabilities.get("reject").copied(),
                    ),
                    _ => (
                        DecisionOutcome::Negative,
                        EvidenceClass::Ambiguous,
                        Some(*confidence),
                        probabilities.get("none").copied(),
                    ),
                }
            }
        }
        RawAnswer::Score {
            score,
            probabilities: _,
            confidence,
            results: _,
        } => {
            check_confidence(*confidence).map_err(|e| PipelineError::Validation(e.to_string()))?;
            if !score.is_finite() {
                return Err(PipelineError::Validation("non-finite score".into()));
            }
            if *confidence < mat.abstain_confidence {
                (
                    DecisionOutcome::Abstained,
                    EvidenceClass::Ambiguous,
                    Some(*confidence),
                    None,
                )
            } else if *score >= mat.accept_score {
                (
                    DecisionOutcome::Accepted,
                    EvidenceClass::Inferred,
                    Some(*confidence),
                    None,
                )
            } else {
                (
                    DecisionOutcome::Negative,
                    EvidenceClass::Ambiguous,
                    Some(*confidence),
                    None,
                )
            }
        }
    };
    let class = if class == EvidenceClass::Inferred && reason == "structural" {
        EvidenceClass::Extracted
    } else {
        class
    };
    Ok((outcome, class, conf, prob))
}
