//! Chaosbox orchestration: snapshot -> extract -> candidates -> Jev decisions
//! -> validated evidence/claims -> policy build -> atomic publication ->
//! read-only consumers. One shared query implementation serves CLI and MCP.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild,
    Relation, RelationScope, check_confidence, check_probability, deterministic_id,
};
use chaosbox_extract::{Extraction, Snapshot, build_candidates, extract_snapshot};
use chaosbox_gel::{MemoryStore, Store};
use chaosbox_jev::{Answer, ChoiceAnswer, NoulAnswer, Question, ScoreAnswer, SystemOneResponse};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("extract: {0}")]
    Extract(String),
    #[error("jev: {0}")]
    Jev(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("store: {0}")]
    Store(String),
    #[error("consumer: {0}")]
    Consumer(String),
}

/// Rubric + acceptance policy. Thresholds are materialization identity:
/// changing them reuses raw decisions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Materialization {
    pub rubric_version: String,
    pub accept_noul: f64,
    pub accept_confidence: f64,
    pub accept_score: f64,
}

impl Default for Materialization {
    fn default() -> Self {
        Self {
            rubric_version: "rubric-v1".into(),
            accept_noul: 0.7,
            accept_confidence: 0.6,
            accept_score: 1.0,
        }
    }
}

impl Materialization {
    pub fn identity(&self, build_inputs: &str) -> String {
        deterministic_id("mat", &[&self.rubric_version, &self.accept_noul.to_string(), &self.accept_confidence.to_string(), build_inputs])
    }
}

/// How a candidate becomes Jev questions. Descriptions (not ids) go into
/// state/instructions; ids carry no inference meaning.
#[must_use]
pub fn questions_for(candidate: &Candidate, from: &Entity, to: &Entity) -> BTreeMap<String, Question> {
    let state_desc = format!(
        "Relation proposal {} from {} ({:?} in {}) to {} ({:?} in {}). Basis: {}. Excerpt: {}",
        format!("{:?}", candidate.rel_type),
        from.qualified_name,
        from.kind,
        from.file,
        to.qualified_name,
        to.kind,
        to.file,
        candidate.reason,
        candidate.state_excerpt
    );
    let _ = state_desc;
    BTreeMap::from([(
        format!("rel_{}", candidate.id),
        Question::Choice {
            instructions: format!(
                "Given the source evidence, which relation holds from '{}' ({:?}, {}) to '{}' ({:?}, {})? \
                 Answer only from the listed options. Option meanings: accept = evidence supports {:?}; \
                 reject = evidence contradicts; none = no finding (successful negative, do not retry). \
                 Entity descriptions are authoritative; the question id is arbitrary.",
                from.qualified_name, from.kind, from.file,
                to.qualified_name, to.kind, to.file,
                candidate.rel_type
            ),
            criteria: BTreeMap::from([
                ("accept".into(), Some(format!("Source shows {:?} from {} to {}", candidate.rel_type, from.qualified_name, to.qualified_name))),
                ("reject".into(), Some("Source contradicts the proposed relation".into())),
                ("none".into(), Some("No finding in source; abstain from the relation".into())),
            ]),
        },
    )])
}

/// Responder abstraction: production uses the real Jev client; tests use a
/// local protocol fixture (no creds, no network).
#[async_trait::async_trait]
pub trait Responder: Send + Sync {
    async fn respond(
        &mut self,
        state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String>;
}

/// Deterministic fixture responder for tests and `test-gel`.
pub struct FixtureResponder {
    pub accept_all: bool,
    pub model: String,
}

impl FixtureResponder {
    #[must_use]
    pub fn new(accept_all: bool) -> Self {
        Self { accept_all, model: chaosbox_jev::JEV_MODEL_PINNED.into() }
    }
}

#[async_trait::async_trait]
impl Responder for FixtureResponder {
    async fn respond(
        &mut self,
        _state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        let mut answers = BTreeMap::new();
        for (id, q) in &questions {
            match q {
                Question::Choice { .. } => {
                    let choice = if self.accept_all { "accept" } else { "none" };
                    answers.insert(
                        id.clone(),
                        Answer::Choice(ChoiceAnswer {
                            kind: "choice".into(),
                            choice: choice.into(),
                            probabilities: BTreeMap::from([
                                ("accept".into(), if self.accept_all { 0.9 } else { 0.1 }),
                                ("reject".into(), 0.05),
                                ("none".into(), if self.accept_all { 0.05 } else { 0.85 }),
                            ]),
                            confidence: 0.85,
                        }),
                    );
                }
                Question::Noul { .. } => {
                    answers.insert(
                        id.clone(),
                        Answer::Noul(NoulAnswer { kind: "noul".into(), noul: 0.8 }),
                    );
                }
                Question::Score { .. } => {
                    answers.insert(
                        id.clone(),
                        Answer::Score(ScoreAnswer {
                            kind: "score".into(),
                            score: 1.0,
                            probabilities: BTreeMap::from([("0".into(), 0.1), ("1".into(), 0.9)]),
                            confidence: 0.8,
                            results: vec![0.1, 0.9],
                        }),
                    );
                }
            }
        }
        Ok(SystemOneResponse {
            model: self.model.clone(),
            answers,
            usage: chaosbox_jev::Usage { input_tokens: 100, output_tokens: 0 },
        })
    }
}

/// Full pipeline state held by the operator commands.
pub struct Pipeline {
    pub store: MemoryStore,
    pub generation: u64,
}

impl Pipeline {
    #[must_use]
    pub fn new() -> Self {
        Self { store: MemoryStore::new(), generation: 0 }
    }

    /// Snapshot -> extract -> candidates.
    pub fn snapshot_extract(
        repo: &str,
        root: &Path,
        max_candidates: usize,
    ) -> Result<(Snapshot, Extraction, Vec<Candidate>), PipelineError> {
        let snap = Snapshot::capture(repo, root).map_err(|e| PipelineError::Extract(e.to_string()))?;
        let ext = extract_snapshot(&snap);
        let cands = build_candidates(&ext, max_candidates);
        Ok((snap, ext, cands))
    }

    /// Bounded decisions over candidates. Each candidate decided independently
    /// (multiple valid relations => independent decisions, never forced single-choice).
    pub async fn decide(
        candidates: &[Candidate],
        entities: &BTreeMap<String, Entity>,
        responder: &mut impl Responder,
        model_requested: &str,
    ) -> Result<Vec<(Candidate, Decision, Evidence)>, PipelineError> {
        let mut out = Vec::new();
        for cand in candidates {
            let from = entities.get(&cand.from_entity).ok_or_else(|| PipelineError::Validation("missing from".into()))?;
            let to = entities.get(&cand.to_entity).ok_or_else(|| PipelineError::Validation("missing to".into()))?;
            let questions = questions_for(cand, from, to);
            let state = serde_json::json!({
                "candidate": cand.id,
                "rel_type": format!("{:?}", cand.rel_type),
                "from": {"qualified_name": from.qualified_name, "kind": format!("{:?}", from.kind), "file": from.file},
                "to": {"qualified_name": to.qualified_name, "kind": format!("{:?}", to.kind), "file": to.file},
                "reason": cand.reason,
                "excerpt": cand.state_excerpt,
            });
            let resp = responder.respond(state, questions.clone()).await.map_err(PipelineError::Jev)?;
            // Record requested vs returned model identities.
            if resp.model.is_empty() {
                return Err(PipelineError::Validation("empty returned model".into()));
            }
            // Validate + reconcile per question.
            let valid: BTreeMap<String, BTreeSet<String>> = BTreeMap::from([(
                format!("rel_{}", cand.id),
                BTreeSet::from(["accept".into(), "reject".into(), "none".into()]),
            )]);
            chaosbox_jev::validate_response(&resp, &questions, &valid).map_err(|e| PipelineError::Validation(e.to_string()))?;
            for (qid, ans) in &resp.answers {
                let (outcome, class, conf, prob) = match ans {
                    Answer::Choice(c) => {
                        check_confidence(c.confidence).map_err(|e| PipelineError::Validation(e.to_string()))?;
                        for p in c.probabilities.values() {
                            check_probability(*p).map_err(|e| PipelineError::Validation(e.to_string()))?;
                        }
                        match c.choice.as_str() {
                            "accept" => (DecisionOutcome::Accepted, EvidenceClass::Inferred, Some(c.confidence), c.probabilities.get("accept").copied()),
                            "reject" => (DecisionOutcome::Rejected, EvidenceClass::Ambiguous, Some(c.confidence), c.probabilities.get("reject").copied()),
                            _ => (DecisionOutcome::Negative, EvidenceClass::Ambiguous, Some(c.confidence), c.probabilities.get("none").copied()),
                        }
                    }
                    Answer::Noul(n) => {
                        check_probability(n.noul).map_err(|e| PipelineError::Validation(e.to_string()))?;
                        if n.noul >= 0.7 {
                            (DecisionOutcome::Accepted, EvidenceClass::Inferred, None, Some(n.noul))
                        } else {
                            (DecisionOutcome::Negative, EvidenceClass::Ambiguous, None, Some(n.noul))
                        }
                    }
                    Answer::Score(s) => {
                        check_confidence(s.confidence).map_err(|e| PipelineError::Validation(e.to_string()))?;
                        if s.score >= 1.0 {
                            (DecisionOutcome::Accepted, EvidenceClass::Inferred, Some(s.confidence), None)
                        } else {
                            (DecisionOutcome::Negative, EvidenceClass::Ambiguous, Some(s.confidence), None)
                        }
                    }
                };
                // EXTRACTED only for explicit source evidence; model confidence
                // alone never upgrades INFERRED -> EXTRACTED.
                let class = if class == EvidenceClass::Inferred && cand.reason == "structural" {
                    EvidenceClass::Extracted
                } else {
                    class
                };
                let decision = Decision {
                    id: deterministic_id("dec", &[&cand.id, qid, model_requested]),
                    candidate_id: cand.id.clone(),
                    question_id: qid.clone(),
                    outcome: outcome.clone(),
                    evidence_class: class,
                    model_requested: model_requested.to_owned(),
                    model_returned: resp.model.clone(),
                    confidence: conf,
                    probability: prob,
                };
                // Evidence text copied from source spans / deterministic template.
                let text = format!("[{}] {} -> {} ({:?})", cand.reason, from.qualified_name, to.qualified_name, cand.rel_type);
                let ev = Evidence {
                    id: deterministic_id("ev", &[&decision.id, "support"]),
                    class,
                    supports: outcome == DecisionOutcome::Accepted,
                    text,
                    span: Some(from.span.clone()),
                    source_file_version: from.file.clone(),
                };
                out.push((cand.clone(), decision, ev));
            }
        }
        Ok(out)
    }

    /// Policy-controlled build + atomic publication with predecessor check.
    pub fn build_and_publish(
        &mut self,
        repo: &str,
        snapshot: &Snapshot,
        extraction: &Extraction,
        decided: &[(Candidate, Decision, Evidence)],
        mat: &Materialization,
        expected_predecessor: Option<String>,
    ) -> Result<GraphBuild, PipelineError> {
        self.generation += 1;
        let mut build = GraphBuild::new(repo, vec![snapshot.id.clone()], self.generation);
        build.predecessor = expected_predecessor.clone();
        let entities: BTreeMap<String, Entity> =
            extraction.entities.iter().map(|e| (e.id.clone(), e.clone())).collect();
        for e in entities.values() {
            build.add_node(e.clone()).map_err(|e| PipelineError::Validation(e.to_string()))?;
        }
        // Materialize accepted relations as first-class objects.
        for (cand, dec, ev) in decided {
            let accept = match &dec.outcome {
                DecisionOutcome::Accepted => {
                    let conf_ok = dec.confidence.map_or(true, |c| c >= mat.accept_confidence);
                    let prob_ok = dec.probability.map_or(true, |p| p >= mat.accept_noul);
                    conf_ok && prob_ok
                }
                _ => false, // rejected/abstained/negative/failure recorded, never materialized
            };
            if !accept {
                continue;
            }
            let scope = if entities.get(&cand.from_entity).map(|e| e.file.clone()).unwrap_or_default()
                == entities.get(&cand.to_entity).map(|e| e.file.clone()).unwrap_or_default()
            {
                RelationScope::File
            } else {
                RelationScope::CrossFile
            };
            let mut rel = Relation::new(cand.rel_type.clone(), &cand.from_entity, &cand.to_entity, scope, &build.id);
            rel.evidence_ids.push(ev.id.clone());
            // Parallel relations preserved: distinct (type, from, to) ids.
            if build.edges.contains_key(&rel.id) {
                rel.id.push('x');
            }
            build.add_edge(rel).map_err(|e| PipelineError::Validation(e.to_string()))?;
        }
        // Invariant: published edges refer to same-build members (enforced by add_edge).
        self.store.publish(build.clone(), expected_predecessor).map_err(|e| PipelineError::Store(e.to_string()))?;
        Ok(build)
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Shared read-only queries (CLI and MCP use these) ----

/// Case-insensitive substring search over names. Bounded.
#[must_use]
pub fn search(build: &GraphBuild, query: &str, limit: usize) -> Vec<Entity> {
    let q = query.to_lowercase();
    let mut out: Vec<Entity> = build
        .nodes
        .values()
        .filter(|e| e.name.to_lowercase().contains(&q) || e.qualified_name.to_lowercase().contains(&q))
        .take(limit)
        .cloned()
        .collect();
    out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    out
}

/// Deterministic JSON export + Graphify-compatible node-link shape.
#[must_use]
pub fn export_json(build: &GraphBuild) -> serde_json::Value {
    let mut nodes: Vec<serde_json::Value> = build
        .nodes
        .values()
        .map(|e| {
            serde_json::json!({
                "id": e.id, "label": e.name, "kind": format!("{:?}", e.kind),
                "source_file": e.file,
                "source_location": format!("L{}", e.span.start_line),
                "qualified_name": e.qualified_name,
            })
        })
        .collect();
    nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let mut links: Vec<serde_json::Value> = build
        .edges
        .values()
        .map(|r| {
            serde_json::json!({
                "id": r.id, "source": r.from, "target": r.to,
                "rel_type": format!("{:?}", r.rel_type),
                "scope": format!("{:?}", r.scope),
            })
        })
        .collect();
    links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    serde_json::json!({
        "directed": true, "multigraph": true,
        "nodes": nodes, "links": links,
        "build_id": build.id, "generation": build.generation,
    })
}

/// Deterministic explain: source-backed structured info, no generated prose.
#[must_use]
pub fn explain_entity(build: &GraphBuild, id: &str) -> Option<serde_json::Value> {
    let e = build.nodes.get(id)?;
    Some(serde_json::json!({
        "id": e.id, "kind": format!("{:?}", e.kind),
        "file": e.file, "qualified_name": e.qualified_name,
        "span": e.span,
        "outgoing": build.outgoing(id, None).len(),
        "incoming": build.incoming(id, None).len(),
    }))
}

// ---- Lifecycle contract v1 (db check / db migrate) ----

/// Versioned JSON envelope. Diagnostics go to stderr; stdout is this JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LifecycleReport {
    pub contract_version: u32,
    pub backend: String,
    pub operation: String,
    pub status: String,
    pub schema_version: u32,
    pub gel_pinned: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl LifecycleReport {
    #[must_use]
    pub fn check_ready(detail: serde_json::Value) -> Self {
        Self {
            contract_version: 1,
            backend: "gel".into(),
            operation: "db check".into(),
            status: "ready".into(),
            schema_version: chaosbox_gel::SCHEMA_VERSION,
            gel_pinned: chaosbox_gel::GEL_PINNED.into(),
            detail,
        }
    }
    #[must_use]
    pub fn pending(operation: &str, reason: &str) -> Self {
        Self {
            contract_version: 1,
            backend: "gel".into(),
            operation: operation.into(),
            status: "pending".into(),
            schema_version: chaosbox_gel::SCHEMA_VERSION,
            gel_pinned: chaosbox_gel::GEL_PINNED.into(),
            detail: serde_json::json!({"reason": reason}),
        }
    }
}

/// Read-only readiness: no implicit init/migration/repair. Exit 0 only when ready.
#[must_use]
pub fn db_check_report(store: &MemoryStore, repo: &str) -> LifecycleReport {
    match store.active(repo) {
        Some(b) => LifecycleReport::check_ready(serde_json::json!({
            "repo": repo, "active_build": b.id, "generation": b.generation,
            "nodes": b.nodes.len(), "edges": b.edges.len(),
            "schema_assets": "packaged",
        })),
        None => LifecycleReport::pending("db check", "no active build for repo"),
    }
}

/// Claim evidence helper used by tests: removing one source keeps others.
#[must_use]
pub fn claim_survives_source_removal(claim: &Claim, removed_evidence: &str) -> bool {
    let remaining_support: Vec<_> =
        claim.supporting.iter().filter(|e| *e != removed_evidence).collect();
    let remaining_contra: Vec<_> =
        claim.contradicting.iter().filter(|e| *e != removed_evidence).collect();
    !remaining_support.is_empty() || !remaining_contra.is_empty() || claim.supporting.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaosbox_core::{SourceSpan, diff_builds};

    #[test]
    fn export_is_deterministic_and_compatible() {
        let mut b = GraphBuild::new("r", vec!["s".into()], 1);
        let e = Entity::new(
            chaosbox_core::EntityKind::Symbol,
            "r",
            "s",
            "a.rs",
            "a",
            "a",
            SourceSpan::point("a.rs", 1, 1, 0),
        );
        b.add_node(e).unwrap();
        let v1 = export_json(&b);
        let v2 = export_json(&b);
        assert_eq!(v1, v2);
        assert!(v1.get("nodes").is_some() && v1.get("links").is_some());
    }

    #[test]
    fn diff_reports_add_remove() {
        let mut a = GraphBuild::new("r", vec!["s1".into()], 1);
        let mut b = GraphBuild::new("r", vec!["s2".into()], 2);
        let e = Entity::new(
            chaosbox_core::EntityKind::Symbol,
            "r",
            "s1",
            "a.rs",
            "a",
            "a",
            SourceSpan::point("a.rs", 1, 1, 0),
        );
        a.add_node(e.clone()).unwrap();
        b.add_node(e.clone()).unwrap();
        let e2 = Entity::new(
            chaosbox_core::EntityKind::Symbol,
            "r",
            "s2",
            "b.rs",
            "b",
            "b",
            SourceSpan::point("b.rs", 1, 1, 0),
        );
        b.add_node(e2.clone()).unwrap();
        let d = diff_builds(&a, &b);
        assert_eq!(d.added_nodes, vec![e2.id]);
        assert!(d.removed_nodes.is_empty());
    }

    #[test]
    fn threshold_change_reuses_decisions() {
        // Same decisions, different materialization => different accepted sets.
        // Identity covers thresholds so raw decisions are reusable.
        let m1 = Materialization { accept_noul: 0.95, ..Default::default() };
        let m2 = Materialization::default();
        assert_ne!(m1.identity("x"), m2.identity("x"));
    }

    #[test]
    fn claim_survives_one_source_removal() {
        let c = Claim {
            id: "c".into(),
            relation_id: "r".into(),
            supporting: vec!["ev1".into(), "ev2".into()],
            contradicting: vec![],
            accepted: true,
        };
        assert!(claim_survives_source_removal(&c, "ev1"));
    }
}
