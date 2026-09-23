//! Materialization identity (rubric and acceptance thresholds) plus the Jev questions a candidate becomes.

use super::{
    Serialize, Deserialize, deterministic_id, PipelineError, Candidate, Entity, BTreeMap, Question,
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
