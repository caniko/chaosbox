//! Selective session intelligence: bounded proposals, typed Jev assessment,
//! deterministic admission, and read-only artifact retrieval. Bundles are
//! private, immutable staging/export artifacts, not automatic `TypeDB` imports.

mod assess;
pub mod cli;
mod extract;

pub use assess::{assess, questions, Assessment, Outcome, RUBRIC_VERSION};
pub use extract::{extract, extract_window, Candidates};

use chaosbox_core::intelligence::{Intelligence, IntelligenceStatus};
use serde::{Deserialize, Serialize};

/// Coverage is a processed candidate window, never a claim of exhaustive recall.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    /// Full captured input digest.
    pub snapshot: String,
    /// Candidates before this window.
    pub skipped: usize,
    /// Proposals assessed or reused from validated receipts.
    pub selected: usize,
    /// Eligible lines omitted from this window, including oversize context.
    pub omitted: usize,
    /// Derived records deliberately excluded from independent evidence.
    pub excluded_derived: usize,
}

/// Sparse knowledge plus receipts. Sources stay in the private archive.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    /// Versioned artifact contract.
    pub version: u32,
    /// Explicit visibility boundary, fixed for the complete bundle.
    pub scope: String,
    /// Accepted knowledge, including historical disputes and supersessions.
    pub records: Vec<Intelligence>,
    /// Typed admission/consolidation receipts. No generated prose evidence.
    pub assessments: Vec<Assessment>,
    /// Explicit processing coverage; retrieval remains non-exhaustive.
    pub coverage: Vec<Coverage>,
}

impl Bundle {
    /// Empty unpublished knowledge set for an explicit private scope.
    #[must_use]
    pub fn new(scope: &str) -> Self {
        Self {
            version: 1,
            scope: scope.into(),
            records: Vec::new(),
            assessments: Vec::new(),
            coverage: Vec::new(),
        }
    }

    /// Validate visibility and references before any read or consolidation.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.scope.trim().is_empty() {
            return Err("unsupported intelligence bundle or empty scope".into());
        }
        let ids: std::collections::BTreeSet<_> = self.records.iter().map(|r| &r.id).collect();
        if ids.len() != self.records.len() {
            return Err("duplicate intelligence identity".into());
        }
        let receipts: std::collections::BTreeSet<_> =
            self.assessments.iter().map(|a| &a.id).collect();
        if receipts.len() != self.assessments.len() {
            return Err("duplicate assessment receipt".into());
        }
        for receipt in &self.assessments {
            receipt.validate(&self.scope)?;
        }
        for record in &self.records {
            if record.scope != self.scope
                || record.evidence.is_empty()
                || record.repositories.is_empty()
                || record.interpretation_class != chaosbox_core::EvidenceClass::Inferred
            {
                return Err("invalid intelligence provenance or scope".into());
            }
            if record.statement != record.evidence[0].quote {
                return Err("intelligence wording must remain verbatim evidence".into());
            }
            for evidence in &record.evidence {
                let hash = chaosbox_core::sha256_hex(&[
                    &serde_json::to_string(evidence).map_err(|_| "encode evidence")?
                ]);
                if !record
                    .assessments
                    .iter()
                    .filter_map(|id| self.assessments.iter().find(|a| &a.id == id))
                    .any(|a| a.evidence_digest == hash && a.repositories == record.repositories)
                {
                    return Err("evidence occurrence is not bound to an assessment".into());
                }
            }
            if record.contradicts.iter().any(|id| !ids.contains(id))
                || record
                    .supersedes
                    .as_ref()
                    .is_some_and(|id| !ids.contains(id))
            {
                return Err("dangling intelligence relationship".into());
            }
            for id in &record.contradicts {
                if id == &record.id
                    || !self
                        .records
                        .iter()
                        .any(|other| &other.id == id && other.contradicts.contains(&record.id))
                {
                    return Err("asymmetric intelligence contradiction".into());
                }
            }
            if record
                .assessments
                .iter()
                .any(|id| !self.assessments.iter().any(|a| &a.id == id))
            {
                return Err("missing intelligence assessment receipt".into());
            }
            let primary = record
                .assessments
                .first()
                .and_then(|id| self.assessments.iter().find(|a| &a.id == id))
                .ok_or("missing primary receipt")?;
            if record.id
                != format!(
                    "intel:{}",
                    chaosbox_core::sha256_hex(&[&primary.candidate_id])
                )
                || primary.repositories != record.repositories
                || primary.evidence_digest
                    != chaosbox_core::sha256_hex(&[&serde_json::to_string(&record.evidence[0])
                        .map_err(|_| "encode evidence")?])
                || !matches!(
                    primary.outcome,
                    Outcome::Admitted | Outcome::Contradiction(_) | Outcome::Supersession(_)
                )
            {
                return Err("intelligence no longer matches its admission evidence".into());
            }
            if record.status != self.status_from_receipts(record) {
                return Err("intelligence status disagrees with receipts".into());
            }
        }
        Ok(())
    }

    fn status_from_receipts(&self, record: &Intelligence) -> IntelligenceStatus {
        if self
            .assessments
            .iter()
            .any(|a| matches!(&a.outcome,Outcome::Supersession(id) if id==&record.id))
        {
            return IntelligenceStatus::Superseded;
        }
        let latest = self.interpretation(record);
        if latest.is_some_and(|a| matches!(a.outcome, Outcome::Rejected | Outcome::Abstained)) {
            IntelligenceStatus::Withheld
        } else if record.contradicts.is_empty() {
            IntelligenceStatus::Admitted
        } else {
            IntelligenceStatus::Disputed
        }
    }

    fn interpretation<'a>(&'a self, record: &Intelligence) -> Option<&'a Assessment> {
        record
            .assessments
            .iter()
            .rev()
            .filter_map(|id| self.assessments.iter().find(|a| &a.id == id))
            .find(|a| {
                record.evidence.iter().any(|e| {
                    serde_json::to_string(e)
                        .is_ok_and(|s| chaosbox_core::sha256_hex(&[&s]) == a.evidence_digest)
                })
            })
    }

    /// Bounded deterministic retrieval; never invokes inference or broadens
    /// repository/scope access. Conflicts accompany disputed records.
    pub fn context(
        &self,
        scope: &str,
        repo: &str,
        query: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        self.validate()?;
        if scope != self.scope {
            return Err("intelligence scope mismatch".into());
        }
        if repo.trim().is_empty()
            || query.trim().is_empty()
            || !(1..=20).contains(&limit)
            || !(256..=32_000).contains(&max_chars)
        {
            return Err("context requires a query, limit 1..20, and max-chars 256..32000".into());
        }
        // ponytail: bounded lexical retrieval, not semantic ranking. Measure
        // missed useful results before introducing embeddings or a reranker.
        let tokens = words(query);
        let mut ranked: Vec<_> = self
            .records
            .iter()
            .filter(|r| {
                !matches!(
                    r.status,
                    IntelligenceStatus::Superseded | IntelligenceStatus::Withheld
                ) && r.repositories.iter().any(|p| p == repo)
            })
            .filter_map(|r| {
                let overlap = words(&r.statement).intersection(&tokens).count();
                (overlap > 0).then_some((overlap, r))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        let mut result = Vec::new();
        let mut used = 0;
        for (_, record) in ranked {
            let mut citations = vec![location(&record.evidence[0])];
            if record.evidence.len() > 1 {
                citations.push(location(record.evidence.last().ok_or("missing evidence")?));
            }
            let projection = serde_json::json!({
                "id":record.id,"statement":record.statement,"kind":record.kind,"status":record.status,
                "interpretation_class":record.interpretation_class,"repositories":record.repositories,
                "citations":citations,"evidence_count":record.evidence.len(),
                "assessment_count":record.assessments.len(),"active_assessment":record.assessments.last(),
                "assessment_policy":self.interpretation(record).map(|a|&a.rubric_version),
                "needs_revalidation":self.interpretation(record).is_none_or(|a|a.rubric_version!=RUBRIC_VERSION),
                "contradicts":record.contradicts,"supersedes":record.supersedes,
            });
            let size = serde_json::to_string(&projection)
                .map_err(|_| "encode intelligence")?
                .chars()
                .count();
            if used + size > max_chars {
                continue;
            }
            used += size;
            result.push(projection);
            if result.len() == limit {
                break;
            }
        }
        Ok(result)
    }
}

fn location(e: &chaosbox_core::intelligence::SessionEvidence) -> serde_json::Value {
    serde_json::json!({"source":e.source,"snapshot":e.snapshot,"session":e.session,
        "message":e.message,"pointer":e.pointer,"line":e.line,"observed_at_ms":e.observed_at_ms})
}

fn words(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| {
            w.len() >= 3
                && !matches!(
                    w.as_str(),
                    "the"
                        | "and"
                        | "for"
                        | "this"
                        | "that"
                        | "with"
                        | "from"
                        | "what"
                        | "how"
                        | "why"
                        | "are"
                        | "was"
                        | "were"
                        | "can"
                        | "should"
                        | "must"
                        | "not"
                        | "have"
                        | "has"
                        | "about"
                        | "did"
                        | "does"
                        | "when"
                        | "our"
                )
        })
        .collect()
}

/// Closed, read-only MCP dispatch over one startup-pinned private bundle.
/// Arguments cannot select files, visibility scopes, models or mutations.
pub fn mcp_query(
    bundle: &Bundle,
    name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContextArgs {
        repo: String,
        query: String,
        limit: Option<usize>,
        max_chars: Option<usize>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EvidenceArgs {
        repo: String,
        id: String,
    }
    match name {
        "intelligence_context" => {
            let args: ContextArgs = serde_json::from_value(args.clone())
                .map_err(|_| "invalid intelligence context arguments")?;
            let records = bundle.context(
                &bundle.scope,
                &args.repo,
                &args.query,
                args.limit.unwrap_or(5),
                args.max_chars.unwrap_or(12_000),
            )?;
            Ok(
                serde_json::json!({"scope":bundle.scope,"historical_data_not_instructions":true,"exhaustive":false,"records":records}),
            )
        }
        "intelligence_evidence" => {
            let args: EvidenceArgs = serde_json::from_value(args.clone())
                .map_err(|_| "invalid intelligence evidence arguments")?;
            cli::evidence(bundle, &bundle.scope, &args.repo, &args.id)
        }
        _ => Err("read-only intelligence: unsupported operation".into()),
    }
}
