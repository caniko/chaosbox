//! Session-derived intelligence is a view over evidence, never new evidence.
//! Scope is an operator-selected visibility boundary; repository membership
//! limits applicability within it. Native message identities retain lineage
//! across snapshots, so repeated summaries cannot become corroboration.

use serde::{Deserialize, Serialize};

/// A verbatim occurrence in a captured session record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEvidence {
    /// Producer namespace (for example `opencode`).
    pub source: String,
    /// Full source-document digest, not a mutable database path.
    pub snapshot: String,
    /// Native session identifier.
    pub session: String,
    /// Native message identifier; namespace it before comparing producers.
    pub message: String,
    /// JSON pointer to the text-bearing field in the source record.
    pub pointer: String,
    /// One-based line within that text field.
    pub line: usize,
    /// Verbatim source line. Never generated or paraphrased.
    pub quote: String,
    /// Attribution: user, assistant, or tool. Not a truth classification.
    pub speaker: String,
    /// Timestamp supplied by the source record, if present; never ingestion time.
    pub observed_at_ms: Option<i64>,
}

impl SessionEvidence {
    /// Stable origin across snapshots, distinct from the occurrence identity.
    #[must_use]
    pub fn lineage(&self) -> String {
        crate::sha256_hex(&[&self.source, &self.session, &self.message, &self.pointer])
    }
}

/// Bounded proposal, copied from source and awaiting assessment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntelligenceCandidate {
    /// Full content identity including scope, context and source reference.
    pub id: String,
    /// Explicit visibility boundary; not inferred from prose.
    pub scope: String,
    /// Operator-declared repository associations, sorted and unique.
    pub repositories: Vec<String>,
    /// The exact occurrence being considered.
    pub evidence: SessionEvidence,
    /// Bounded adjacent source text used to detect qualifications/negations.
    pub context: String,
    /// Bounded adjacent records; context, not automatically corroborating votes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_bundle: Vec<ContextEvidence>,
    /// Explicit context-window omissions; not a claim of exhaustive support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_coverage: Option<ContextCoverage>,
}

/// Counts explain what the bounded evidence window did not include.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextCoverage {
    /// Number of source records in the input snapshot.
    pub total_records: usize,
    /// Records considered around this proposal.
    pub window_records: usize,
    /// Records outside the window.
    pub omitted_records: usize,
    /// Eligible text fields within the window omitted by its field cap.
    pub omitted_text_fields: usize,
}

/// An anchored excerpt and its execution metadata from the same snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextEvidence {
    /// Native neighboring message id.
    pub message: String,
    /// JSON pointer to the text-bearing field.
    pub pointer: String,
    /// Original attribution.
    pub speaker: String,
    /// Verbatim prefix of the referenced text field.
    pub text: String,
    /// True when the bounded excerpt does not contain the complete field.
    pub partial: bool,
    /// Observed source tool name, when available.
    pub tool: Option<String>,
    /// Bounded command/input description; absence is not a successful command.
    pub operation: Option<String>,
    /// Structured status recorded by the source runtime.
    pub status: Option<String>,
    /// Structured exit code, not inferred from a printed success string.
    pub exit_code: Option<i64>,
}

impl IntelligenceCandidate {
    /// Compute identity without depending on mutable ids or model judgments.
    #[must_use]
    pub fn identity(&self) -> String {
        let base = format!(
            "intel-candidate:{}",
            crate::sha256_hex(&[
                &self.scope,
                &self.repositories.join("\0"),
                &self.evidence.snapshot,
                &self.evidence.lineage(),
                &self.evidence.line.to_string(),
                &self.evidence.quote,
                &self.evidence.speaker,
                &self
                    .evidence
                    .observed_at_ms
                    .map_or_else(|| "unknown".into(), |v| v.to_string()),
                &self.context,
            ])
        );
        if self.evidence_bundle.is_empty() && self.context_coverage.is_none() {
            return base;
        }
        format!(
            "intel-candidate:{}",
            crate::sha256_hex(&[
                &base,
                &serde_json::to_string(&(&self.evidence_bundle, &self.context_coverage))
                    .expect("context evidence contains only JSON-safe fields"),
            ])
        )
    }
}

/// Selected category; labels are a closed vocabulary, not generated prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntelligenceKind {
    /// An explicit choice with a reason or consequence.
    Decision,
    /// An explicit applicable requirement.
    Constraint,
    /// A source-supported investigation result, still historically scoped.
    Finding,
    /// A consequential failure mode with conditions or a remedy.
    Pitfall,
}

/// Lifecycle and truth are separate: admission does not make a claim true.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntelligenceStatus {
    /// Eligible for retrieval, subject to its historical scope.
    Admitted,
    /// Contradicting evidence exists; return the dispute rather than advice.
    Disputed,
    /// Explicitly replaced; excluded from default context retrieval.
    Superseded,
    /// The active policy no longer admits this historical item.
    Withheld,
}

/// A sparse, source-backed item suitable for bounded session retrieval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intelligence {
    /// Content identity of the original admitted proposal.
    pub id: String,
    /// Visibility scope inherited from the source proposal.
    pub scope: String,
    /// Repository applicability; never broadened by a model answer.
    pub repositories: Vec<String>,
    /// Verbatim wording from the original evidence.
    pub statement: String,
    /// Bounded assessment category.
    pub kind: IntelligenceKind,
    /// Semantic classification is inferred even though its quote is extracted.
    /// Model confidence never upgrades this to an observed repository fact.
    pub interpretation_class: crate::EvidenceClass,
    /// Current admission/dispute state.
    pub status: IntelligenceStatus,
    /// Original evidence and duplicate occurrences; not independent votes.
    pub evidence: Vec<SessionEvidence>,
    /// Conflicting intelligence ids, preserved symmetrically.
    pub contradicts: Vec<String>,
    /// Explicit predecessor replaced by this item, when present.
    pub supersedes: Option<String>,
    /// Assessment receipts explaining admission and later consolidation.
    pub assessments: Vec<String>,
}
