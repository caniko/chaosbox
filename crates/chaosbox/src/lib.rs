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

/// Responder abstraction: production uses the real Jev client; tests use a
/// local protocol fixture (no creds, no network).
#[async_trait::async_trait]
pub trait Responder: Send + Sync {
    /// Answer one batch of questions; production uses the Jev HTTP client.
    async fn respond(
        &mut self,
        state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String>;
}

/// Deterministic fixture responder for tests and `test-typedb`.
pub struct FixtureResponder {
    /// When true every Choice answer is `accept`; otherwise `none`.
    pub accept_all: bool,
    /// Reported model identity (defaults to the pinned Jev model).
    pub model: String,
}

impl FixtureResponder {
    /// A deterministic responder for tests and credential-free runs.
    #[must_use]
    pub fn new(accept_all: bool) -> Self {
        Self {
            accept_all,
            model: chaosbox_jev::JEV_MODEL_PINNED.into(),
        }
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
                    answers.insert(id.clone(), Answer::Noul(NoulAnswer { noul: 0.8 }));
                }
                Question::Score { .. } => {
                    answers.insert(
                        id.clone(),
                        Answer::Score(ScoreAnswer {
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
            usage: chaosbox_jev::Usage {
                input_tokens: 100,
                output_tokens: 0,
            },
        })
    }
}

/// Live responder: drives the real [`JevClient`] HTTP adapter.
/// Valid options come from each question's own criteria keys, so candidate
/// membership is enforced on live answers exactly as in tests.
pub struct LiveResponder {
    client: JevClient,
}

impl LiveResponder {
    /// Wrap a configured client (endpoint, model, budgets, retries).
    #[must_use]
    pub fn new(client: JevClient) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Responder for LiveResponder {
    async fn respond(
        &mut self,
        state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        let valid: BTreeMap<String, BTreeSet<String>> = questions
            .iter()
            .map(|(id, q)| match q {
                Question::Choice { criteria, .. } => {
                    (id.clone(), criteria.keys().cloned().collect())
                }
                Question::Noul { .. } | Question::Score { .. } => (id.clone(), BTreeSet::new()),
            })
            .collect();
        self.client
            .evaluate(state, questions, &valid)
            .await
            .map_err(|e| e.to_string())
    }
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

/// Full pipeline state held by the operator commands.
/// Generic over [`chaosbox_store::Store`] with [`MemoryStore`] as the default.
pub struct Pipeline<S = MemoryStore> {
    /// Backing store (decisions, evidence, builds, active pointer).
    pub store: S,
    /// Local generation counter for builds published through this pipeline.
    pub generation: u64,
}

/// Count candidates whose cached decision cannot be reused: a key miss, a
/// changed cache key, or a recorded `Failed` outcome (retries always
/// re-ask). Each such candidate costs exactly one live Jev request, so
/// operators can check `uncached <= max_requests` **before** any spend —
/// the budget preflight in `run --live-jev`.
///
/// The cache test mirrors [`Pipeline::decide`] verbatim (same catalog
/// digest, question set, model and rubric inputs); if decide's reuse rule
/// changes, this function must change with it.
pub async fn uncached_decisions<S: chaosbox_store::Store>(
    candidates: &[Candidate],
    entities: &BTreeMap<String, Entity>,
    model_requested: &str,
    mat: &Materialization,
    store: &S,
) -> Result<usize, PipelineError> {
    mat.validate()?;
    let catalog = catalog_digest(candidates);
    let mut uncached = 0usize;
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
        match store
            .find_decision(&cand.id, &qid)
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?
        {
            Some(stored)
                if stored.cache_key == key
                    && !matches!(stored.outcome, DecisionOutcome::Failed(_)) => {}
            _ => uncached += 1,
        }
    }
    Ok(uncached)
}

impl<S: chaosbox_store::Store + Default> Pipeline<S> {
    /// A pipeline with an empty store at generation zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: S::default(),
            generation: 0,
        }
    }

    /// Snapshot -> extract -> candidates (with truncation accounting).
    pub fn snapshot_extract(
        repo: &str,
        root: &Path,
        max_candidates: usize,
    ) -> Result<(Snapshot, Extraction, CandidateCatalog), PipelineError> {
        let snap =
            Snapshot::capture(repo, root).map_err(|e| PipelineError::Extract(e.to_string()))?;
        let ext = extract_snapshot(&snap);
        let catalog = build_candidates(&ext, max_candidates);
        Ok((snap, ext, catalog))
    }

    /// Bounded decisions over candidates. Each candidate decided independently
    /// (multiple valid relations => independent decisions, never forced single-choice).
    /// Preliminary outcome cutoffs come from `mat` so decision and
    /// publication share one threshold source; the materialization identity
    /// covers every threshold, keeping raw decisions reusable.
    /// Every decision and its evidence is persisted to `store` as produced
    /// (the `TypeDB` store writes decisions through to the server, so a dead
    /// worker or a later budget failure loses nothing already paid for),
    /// so a dead worker loses nothing already decided. Claims are assembled
    /// later in [`Pipeline::build_and_publish`], where relation ids exist.
    // Long decision pipeline; splitting stages apart is the owning
    // session's refactor. Allowed to keep CI unblocked.
    #[allow(clippy::too_many_lines)]
    pub async fn decide(
        candidates: &[Candidate],
        entities: &BTreeMap<String, Entity>,
        responder: &mut impl Responder,
        model_requested: &str,
        mat: &Materialization,
        store: &mut S,
    ) -> Result<Vec<(Candidate, Decision, Evidence)>, PipelineError> {
        mat.validate()?;
        // Conservative cache identity: the whole-catalog digest feeds every
        // key, so any catalog change re-asks all decisions (documented).
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
            // Cache reuse: same key and never a recorded failure (retries
            // always re-ask). Evidence rebuilds byte-identically.
            if let Some(stored) = store
                .find_decision(&cand.id, &qid)
                .await
                .map_err(|e| PipelineError::Store(e.to_string()))?
            {
                if stored.cache_key == key && !matches!(stored.outcome, DecisionOutcome::Failed(_))
                {
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
                    continue;
                }
            }
            let state = serde_json::json!({
                "candidate": cand.id,
                "rel_type": format!("{:?}", cand.rel_type),
                "from": {"qualified_name": from.qualified_name, "kind": format!("{:?}", from.kind), "file": from.file},
                "to": {"qualified_name": to.qualified_name, "kind": format!("{:?}", to.kind), "file": to.file},
                "reason": cand.reason,
                "excerpt": cand.state_excerpt,
            });
            // Per-candidate faults become recorded Failed decisions (retryable),
            // never batch aborts and never retried blindly as empty responses.
            // The error text is NOT copied into evidence (untrusted responder).
            let Ok(r) = responder.respond(state, questions.clone()).await else {
                let key = cache_key(
                    &from.snapshot,
                    &catalog,
                    &questions,
                    model_requested,
                    &mat.rubric_version,
                );
                let decision = Decision {
                    id: deterministic_id("dec", &[&cand.id, "failed", model_requested]),
                    candidate_id: cand.id.clone(),
                    question_id: format!("rel_{}", cand.id),
                    outcome: DecisionOutcome::Failed("responder fault".into()),
                    evidence_class: EvidenceClass::Ambiguous,
                    model_requested: model_requested.to_owned(),
                    model_returned: String::new(),
                    confidence: None,
                    probability: None,
                    cache_key: key,
                };
                let ev = assemble_evidence(
                    &decision,
                    false,
                    "decision attempt failed; see attempt accounting".into(),
                    None,
                    &from.snapshot,
                    &from.file,
                    "failed",
                );
                store
                    .put_decision(decision.clone())
                    .await
                    .map_err(|e| PipelineError::Store(e.to_string()))?;
                store
                    .put_evidence(ev.clone())
                    .await
                    .map_err(|e| PipelineError::Store(e.to_string()))?;
                out.push((cand.clone(), decision, ev));
                continue;
            };
            let resp = r;
            // Record requested vs returned model identities.
            if resp.model.is_empty() {
                return Err(PipelineError::Validation("empty returned model".into()));
            }
            // Validate + reconcile per question.
            let valid: BTreeMap<String, BTreeSet<String>> = BTreeMap::from([(
                format!("rel_{}", cand.id),
                BTreeSet::from(["accept".into(), "reject".into(), "none".into()]),
            )]);
            chaosbox_jev::validate_response(&resp, &questions, &valid)
                .map_err(|e| PipelineError::Validation(e.to_string()))?;
            for (qid, ans) in &resp.answers {
                // Abstain-first precedence: below-floor confidence abstains
                // regardless of the selected option or score.
                let (outcome, class, conf, prob) = match ans {
                    Answer::Choice(c) => {
                        check_confidence(c.confidence)
                            .map_err(|e| PipelineError::Validation(e.to_string()))?;
                        for p in c.probabilities.values() {
                            check_probability(*p)
                                .map_err(|e| PipelineError::Validation(e.to_string()))?;
                        }
                        if c.confidence < mat.abstain_confidence {
                            (
                                DecisionOutcome::Abstained,
                                EvidenceClass::Ambiguous,
                                Some(c.confidence),
                                None,
                            )
                        } else {
                            match c.choice.as_str() {
                                "accept" => (
                                    DecisionOutcome::Accepted,
                                    EvidenceClass::Inferred,
                                    Some(c.confidence),
                                    c.probabilities.get("accept").copied(),
                                ),
                                "reject" => (
                                    DecisionOutcome::Rejected,
                                    EvidenceClass::Ambiguous,
                                    Some(c.confidence),
                                    c.probabilities.get("reject").copied(),
                                ),
                                _ => (
                                    DecisionOutcome::Negative,
                                    EvidenceClass::Ambiguous,
                                    Some(c.confidence),
                                    c.probabilities.get("none").copied(),
                                ),
                            }
                        }
                    }
                    Answer::Noul(n) => {
                        check_probability(n.noul)
                            .map_err(|e| PipelineError::Validation(e.to_string()))?;
                        if n.noul >= mat.accept_noul {
                            (
                                DecisionOutcome::Accepted,
                                EvidenceClass::Inferred,
                                None,
                                Some(n.noul),
                            )
                        } else {
                            (
                                DecisionOutcome::Negative,
                                EvidenceClass::Ambiguous,
                                None,
                                Some(n.noul),
                            )
                        }
                    }
                    Answer::Score(s) => {
                        check_confidence(s.confidence)
                            .map_err(|e| PipelineError::Validation(e.to_string()))?;
                        if s.confidence < mat.abstain_confidence {
                            (
                                DecisionOutcome::Abstained,
                                EvidenceClass::Ambiguous,
                                Some(s.confidence),
                                None,
                            )
                        } else if s.score >= mat.accept_score {
                            (
                                DecisionOutcome::Accepted,
                                EvidenceClass::Inferred,
                                Some(s.confidence),
                                None,
                            )
                        } else {
                            (
                                DecisionOutcome::Negative,
                                EvidenceClass::Ambiguous,
                                Some(s.confidence),
                                None,
                            )
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
                    cache_key: cache_key(
                        &from.snapshot,
                        &catalog,
                        &questions,
                        model_requested,
                        &mat.rubric_version,
                    ),
                };
                // Evidence text copied from source spans / deterministic template.
                let text = format!(
                    "[{}] {} -> {} ({:?})",
                    cand.reason, from.qualified_name, to.qualified_name, cand.rel_type
                );
                let supports = outcome == DecisionOutcome::Accepted;
                let ev = assemble_evidence(
                    &decision,
                    supports,
                    text,
                    Some(from.span.clone()),
                    &from.snapshot,
                    &from.file,
                    "support",
                );
                store
                    .put_decision(decision.clone())
                    .await
                    .map_err(|e| PipelineError::Store(e.to_string()))?;
                store
                    .put_evidence(ev.clone())
                    .await
                    .map_err(|e| PipelineError::Store(e.to_string()))?;
                out.push((cand.clone(), decision, ev));
            }
        }
        Ok(out)
    }

    /// Policy-controlled build + atomic publication with predecessor check.
    /// Async because the live backend needs network IO for the flush.
    ///
    /// Failed decisions (responder faults, exhausted budgets) never publish:
    /// a partial graph would displace the last good active build while
    /// reporting success. Fix the backend and re-run instead.
    pub async fn build_and_publish(
        &mut self,
        repo: &str,
        snapshot: &Snapshot,
        extraction: &Extraction,
        decided: &[(Candidate, Decision, Evidence)],
        mat: &Materialization,
        expected_predecessor: Option<String>,
    ) -> Result<GraphBuild, PipelineError> {
        reject_failed(decided)?;
        self.generation += 1;
        let mut build = GraphBuild::new(repo, vec![snapshot.id.clone()], self.generation);
        build.predecessor = expected_predecessor.clone();
        let entities: BTreeMap<String, Entity> = extraction
            .entities
            .iter()
            .map(|e| (e.id.clone(), e.clone()))
            .collect();
        for e in entities.values() {
            build
                .add_node(e.clone())
                .map_err(|e| PipelineError::Validation(e.to_string()))?;
        }
        // Materialize accepted relations as first-class objects.
        // Index rejected evidence by endpoint triple so materialized claims
        // carry their same-batch contradicting evidence.
        let mut contradictions: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
        for (cand, dec, ev) in decided {
            if dec.outcome == DecisionOutcome::Rejected {
                contradictions
                    .entry((
                        cand.from_entity.clone(),
                        cand.to_entity.clone(),
                        format!("{:?}", cand.rel_type),
                    ))
                    .or_default()
                    .push(ev.id.clone());
            }
        }
        for (cand, dec, ev) in decided {
            let accept = match &dec.outcome {
                DecisionOutcome::Accepted => {
                    let conf_ok = dec.confidence.is_none_or(|c| c >= mat.accept_confidence);
                    let prob_ok = dec.probability.is_none_or(|p| p >= mat.accept_noul);
                    conf_ok && prob_ok
                }
                _ => false, // rejected/abstained/negative/failure recorded, never materialized
            };
            if !accept {
                continue;
            }
            let scope = if entities
                .get(&cand.from_entity)
                .map(|e| e.file.clone())
                .unwrap_or_default()
                == entities
                    .get(&cand.to_entity)
                    .map(|e| e.file.clone())
                    .unwrap_or_default()
            {
                RelationScope::File
            } else {
                RelationScope::CrossFile
            };
            let mut rel = Relation::new(
                cand.rel_type.clone(),
                &cand.from_entity,
                &cand.to_entity,
                scope,
                &build.id,
            );
            rel.evidence_ids.push(ev.id.clone());
            // Parallel relations preserved: distinct (type, from, to) ids get
            // a deterministic numeric suffix so N-way collisions all survive.
            if build.edges.contains_key(&rel.id) {
                let base = rel.id.clone();
                let mut n = 2u32;
                loop {
                    rel.id = format!("{base}#{n}");
                    if !build.edges.contains_key(&rel.id) {
                        break;
                    }
                    n += 1;
                }
            }
            build
                .add_edge(rel.clone())
                .map_err(|e| PipelineError::Validation(e.to_string()))?;
            // One claim per materialized relation: supporting evidence from
            // the accepted decision, contradicting evidence from same-batch
            // rejections over the identical triple (empty when none).
            let triple = (
                cand.from_entity.clone(),
                cand.to_entity.clone(),
                format!("{:?}", cand.rel_type),
            );
            let claim = Claim {
                id: deterministic_id("claim", &[&rel.id]),
                relation_id: rel.id.clone(),
                supporting: vec![ev.id.clone()],
                contradicting: contradictions.get(&triple).cloned().unwrap_or_default(),
                accepted: true,
            };
            self.store
                .put_claim(claim)
                .await
                .map_err(|e| PipelineError::Store(e.to_string()))?;
        }
        // Invariant: published edges refer to same-build members (enforced by add_edge).
        self.store
            .publish(build.clone(), expected_predecessor)
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
        Ok(build)
    }
}

impl<S: chaosbox_store::Store + Default> Default for Pipeline<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome counts for one decided batch, for operator reporting.
/// Keys: `accepted`, `rejected`, `abstained`, `negative`, `failed`.
#[must_use]
pub fn summarize_outcomes(
    decided: &[(Candidate, Decision, Evidence)],
) -> BTreeMap<&'static str, usize> {
    let mut out: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_, d, _) in decided {
        let key = match d.outcome {
            DecisionOutcome::Accepted => "accepted",
            DecisionOutcome::Rejected => "rejected",
            DecisionOutcome::Abstained => "abstained",
            DecisionOutcome::Negative => "negative",
            DecisionOutcome::Failed(_) => "failed",
        };
        *out.entry(key).or_default() += 1;
    }
    out
}

/// Reject batches containing failed decisions before publication: a partial
/// graph must not displace the last good active build while reporting
/// success. Fix the responder and re-run instead.
fn reject_failed(decided: &[(Candidate, Decision, Evidence)]) -> Result<(), PipelineError> {
    let failed = decided
        .iter()
        .filter(|(_, d, _)| matches!(d.outcome, DecisionOutcome::Failed(_)))
        .count();
    if failed > 0 {
        return Err(PipelineError::Validation(format!(
            "{failed} failed decisions; refusing to publish (retry once the responder is healthy)"
        )));
    }
    Ok(())
}

/// Next publication chain for a fresh process: the live active build (if
/// any) becomes the expected predecessor, and the pipeline's starting
/// generation becomes the live generation so [`Pipeline::build_and_publish`]
/// mints exactly one generation higher. The store still re-validates live
/// state at swing time, so this only fixes the fresh-process default — it
/// never weakens the exactly-once guard.
pub fn chain_publication(
    active: Option<(String, i64)>,
) -> Result<(Option<String>, u64), PipelineError> {
    let Some((build_id, generation)) = active else {
        return Ok((None, 0));
    };
    let starting = u64::try_from(generation).map_err(|_| {
        PipelineError::Validation(format!("live generation out of range: {generation}"))
    })?;
    // build_and_publish increments, so the mint must fit one higher.
    starting.checked_add(1).ok_or_else(|| {
        PipelineError::Validation(format!("live generation out of range: {generation}"))
    })?;
    Ok((Some(build_id), starting))
}

// ---- Shared read-only queries (CLI and MCP use these) ----

/// Case-insensitive substring search over names. Bounded: sorts all matches
/// by qualified name, then takes the first `limit`.
#[must_use]
pub fn search(build: &GraphBuild, query: &str, limit: usize) -> Vec<Entity> {
    let q = query.to_lowercase();
    let mut out: Vec<Entity> = build
        .nodes
        .values()
        .filter(|e| {
            e.name.to_lowercase().contains(&q) || e.qualified_name.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    out.truncate(limit);
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

// ---- Lifecycle contract (db check / db migrate) ----

/// Versioned JSON envelope. Diagnostics go to stderr; stdout is this JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LifecycleReport {
    /// Contract version (currently 2).
    pub contract_version: u32,
    /// Storage backend label (`typedb`).
    pub backend: String,
    /// Operation name (`db check`, `db migrate`).
    pub operation: String,
    /// `ready`, `pending`, or `error`.
    pub status: String,
    /// Schema compatibility marker.
    pub schema_version: u32,
    /// Pinned backend version the schema targets.
    pub pinned: String,
    /// Sanitized machine-readable detail (never secret values).
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl LifecycleReport {
    /// Shared builder (contract v2): the envelope carries the pinned
    /// `TypeDB` version the schema targets.
    fn report(operation: &str, status: &str, detail: serde_json::Value) -> Self {
        Self {
            contract_version: 2,
            backend: "typedb".into(),
            operation: operation.into(),
            status: status.into(),
            schema_version: chaosbox_typedb::SCHEMA_VERSION,
            pinned: chaosbox_typedb::TYPEDB_PINNED.into(),
            detail,
        }
    }

    /// A ready report: exit 0 after the caller prints it.
    #[must_use]
    pub fn check_ready(detail: serde_json::Value) -> Self {
        Self::report("db check", "ready", detail)
    }

    /// A non-ready report: the caller prints it and exits nonzero.
    #[must_use]
    pub fn pending(operation: &str, reason: &str) -> Self {
        Self::report(operation, "pending", serde_json::json!({"reason": reason}))
    }

    /// An operational-error report: sanitized diagnostics, stdout stays parseable.
    #[must_use]
    pub fn error(operation: &str, reason: &str) -> Self {
        Self::report(operation, "error", serde_json::json!({"reason": reason}))
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

// ---- Read-only consumer path (CLI and MCP share this) ----

/// Escape LIKE wildcards (`\`, `%`, `_`) so user input matches literally.
/// Shared by both backends: the live path relies on backslash LIKE escapes.
fn escape_like(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for c in query.chars() {
        if c == '\\' || c == '%' || c == '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Relation vocabulary for relationship-type filters; an empty filter matches nothing, so
/// callers pass [`all_relation_types`] for unfiltered neighborhoods.
#[must_use]
pub fn all_relation_types() -> Vec<String> {
    [
        "contains",
        "defines",
        "imports",
        "references",
        "calls",
        "links_to",
        "mentions",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Validate a relation-type filter against the vocabulary (case-insensitive),
/// returning canonical names. Unknown names error loudly with the valid list
/// instead of silently matching nothing (empty means "no relations").
pub fn validate_rel_filter(
    filter: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, PipelineError> {
    match filter {
        None => Ok(None),
        Some(names) => {
            let vocab = all_relation_types();
            let mut out = Vec::with_capacity(names.len());
            for n in names {
                match vocab.iter().find(|v| v.eq_ignore_ascii_case(&n)) {
                    Some(canonical) => out.push(canonical.clone()),
                    None => {
                        return Err(PipelineError::Consumer(format!(
                            "unknown relation type {n:?}; valid: {}",
                            vocab.join(", ")
                        )));
                    }
                }
            }
            Ok(Some(out))
        }
    }
}

/// Hard cap for export projections; truncation is reported, never silent.
pub const EXPORT_NODE_CAP: i64 = 10_000;
/// Hard cap for exported edges.
pub const EXPORT_EDGE_CAP: i64 = 20_000;

/// Backend read-only queries. One build id is pinned per reader from the
/// active-build pointer; readers never mutate, migrate, or load Jev credentials.
/// Generic over [`chaosbox_store::GraphQueries`] with `TypeDbReader` as the
/// live backend and [`chaosbox_store::MemoryReader`] for tests.
pub struct GraphReader<R> {
    handle: R,
    /// Pinned active build id for every request this reader serves.
    pub build_id: String,
    /// Pinned generation (predecessor/generation checks on the read side).
    pub generation: i64,
    /// Pinned build status (`active`; the pointer can only pin active builds).
    pub status: String,
    /// Snapshot ids pinned by the build: the freshness fingerprint status
    /// reports so consumers can detect a build that no longer matches its
    /// sources. Empty when the backend's projection predates the field.
    pub snapshots: Vec<String>,
}

impl<R: chaosbox_store::GraphQueries> GraphReader<R> {
    /// Pin the active build for `repo` on an existing query backend.
    /// Used by tests with [`chaosbox_store::MemoryReader`].
    pub async fn pinned(handle: R, repo: &str) -> Result<Self, PipelineError> {
        let build = handle
            .active_build(repo)
            .await
            .map_err(|e| PipelineError::Consumer(format!("active build: {e}")))?
            .ok_or_else(|| PipelineError::Consumer(format!("no active build for repo {repo}")))?;
        Ok(Self {
            handle,
            build_id: build.build_id,
            generation: build.generation,
            status: build.status,
            snapshots: build.snapshots,
        })
    }

    /// Bounded substring search over the pinned build's entity names.
    /// LIKE wildcards in the query are escaped: they match literally.
    pub async fn search(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<chaosbox_store::EntityRow>, PipelineError> {
        let like = format!("%{}%", escape_like(query));
        self.handle
            .search_entities(&self.build_id, &like, limit)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))
    }

    /// Typed entity lookup within the pinned build.
    pub async fn lookup(
        &self,
        id: &str,
    ) -> Result<Option<chaosbox_store::EntityRow>, PipelineError> {
        self.handle
            .entity_by_id(&self.build_id, id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))
    }

    /// Incoming/outgoing neighborhoods within the pinned build, with an
    /// optional relation filter. `None` means all relation types.
    pub async fn neighbors(
        &self,
        id: &str,
        filter: Option<Vec<String>>,
    ) -> Result<(Vec<chaosbox_store::RelRow>, Vec<chaosbox_store::RelRow>), PipelineError> {
        let types = validate_rel_filter(filter)?.unwrap_or_else(all_relation_types);
        let out = self
            .handle
            .neighbors_out(&self.build_id, id, types.clone())
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        let inc = self
            .handle
            .neighbors_in(&self.build_id, id, types)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        Ok((out, inc))
    }

    /// Upper bound on BFS node visits for one path query. Exceeding it is
    /// an explicit budget error, never an unbounded traversal: callers
    /// (MCP agents) retry with a narrower query instead of hanging the
    /// serial server loop.
    pub const PATH_VISITED_CAP: usize = 100_000;

    /// Bounded BFS path using iterative backend neighborhood expansion.
    /// `ponytail: O(hops * degree) round-trips; single-projection fetch if this dominates`.
    pub async fn path(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        self.path_with_cap(from, to, max_hops, Self::PATH_VISITED_CAP)
            .await
    }

    /// [`GraphReader::path`] with an explicit visit budget (tests + future policy).
    pub async fn path_with_cap(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
        visit_cap: usize,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        if from == to {
            return Ok(Some(vec![from.to_owned()]));
        }
        let types = all_relation_types();
        let mut prev: BTreeMap<String, String> = BTreeMap::new();
        let mut seen: BTreeSet<String> = BTreeSet::from([from.to_owned()]);
        let mut queue: VecDeque<(String, usize)> = VecDeque::from([(from.to_owned(), 0)]);
        while let Some((cur, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            let (out, inc) = self.neighbors(&cur, Some(types.clone())).await?;
            let mut nexts: Vec<String> = Vec::new();
            for r in out.iter().chain(inc.iter()) {
                nexts.push(r.from_entity.entity_id.clone());
                nexts.push(r.to_entity.entity_id.clone());
            }
            for nxt in nexts {
                if nxt == cur || !seen.insert(nxt.clone()) {
                    continue;
                }
                if seen.len() > visit_cap {
                    return Err(PipelineError::Consumer(
                        "path traversal budget exceeded; narrow the query".into(),
                    ));
                }
                prev.insert(nxt.clone(), cur.clone());
                if nxt == to {
                    let mut path = vec![to.to_owned()];
                    let mut c = to.to_owned();
                    while let Some(p) = prev.get(&c) {
                        path.push(p.clone());
                        c = p.clone();
                    }
                    path.reverse();
                    return Ok(Some(path));
                }
                queue.push_back((nxt, depth + 1));
            }
        }
        Ok(None)
    }

    /// Deterministic export of the pinned build; truncation errors honestly.
    pub async fn export(&self) -> Result<serde_json::Value, PipelineError> {
        let entities = self
            .handle
            .build_entities(&self.build_id, EXPORT_NODE_CAP + 1)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        if i64::try_from(entities.len()).expect("entity count fits in i64") > EXPORT_NODE_CAP {
            return Err(PipelineError::Consumer(format!(
                "export truncated at {EXPORT_NODE_CAP} nodes; narrow the repo"
            )));
        }
        let rels = self
            .handle
            .build_relationships(&self.build_id, EXPORT_EDGE_CAP + 1)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        if i64::try_from(rels.len()).expect("relation count fits in i64") > EXPORT_EDGE_CAP {
            return Err(PipelineError::Consumer(format!(
                "export truncated at {EXPORT_EDGE_CAP} edges; narrow the repo"
            )));
        }
        let mut nodes: Vec<serde_json::Value> = entities
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.entity_id, "label": e.name, "kind": e.kind,
                    "source_file": e.file, "qualified_name": e.qualified_name,
                })
            })
            .collect();
        nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let mut links: Vec<serde_json::Value> = rels
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.rel_id, "source": r.from_entity.entity_id,
                    "target": r.to_entity.entity_id, "rel_type": r.rel_type,
                })
            })
            .collect();
        links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(serde_json::json!({
            "directed": true, "multigraph": true,
            "nodes": nodes, "links": links,
            "build_id": self.build_id, "generation": self.generation,
        }))
    }

    /// Claim evidence and source locations for one relationship of the
    /// pinned build.
    pub async fn evidence(&self, rel_id: &str) -> Result<serde_json::Value, PipelineError> {
        let rows = self
            .handle
            .evidence_for(&self.build_id, rel_id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        Ok(serde_json::json!({"rel": rel_id, "evidence": rows}))
    }

    /// Source-backed entity explanation: structured info, no generated prose.
    pub async fn explain(&self, id: &str) -> Result<serde_json::Value, PipelineError> {
        let entity = self
            .handle
            .entity_by_id(&self.build_id, id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        let Some(e) = entity else {
            return Ok(serde_json::Value::Null);
        };
        let (out, inc) = self.neighbors(id, None).await?;
        Ok(serde_json::json!({
            "id": e.entity_id, "kind": e.kind, "file": e.file,
            "qualified_name": e.qualified_name,
            "outgoing": out.len(), "incoming": inc.len(),
        }))
    }

    /// Node/edge id diff between two builds of one repo, bounded by the
    /// export caps on each side.
    pub async fn diff(
        &self,
        repo: &str,
        from_build: &str,
        to_build: &str,
    ) -> Result<serde_json::Value, PipelineError> {
        use std::collections::BTreeSet;
        async fn members<R2: chaosbox_store::GraphQueries>(
            reader: &GraphReader<R2>,
            build: &str,
        ) -> Result<(BTreeSet<String>, BTreeSet<String>), PipelineError> {
            let ents = reader
                .handle
                .build_entities(build, EXPORT_NODE_CAP + 1)
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?;
            let rels = reader
                .handle
                .build_relationships(build, EXPORT_EDGE_CAP + 1)
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?;
            if i64::try_from(ents.len()).expect("entity count fits in i64") > EXPORT_NODE_CAP
                || i64::try_from(rels.len()).expect("relation count fits in i64") > EXPORT_EDGE_CAP
            {
                return Err(PipelineError::Consumer(
                    "diff truncated at export caps".into(),
                ));
            }
            Ok((
                ents.iter().map(|e| e.entity_id.clone()).collect(),
                rels.iter().map(|r| r.rel_id.clone()).collect(),
            ))
        }
        let (old_n, old_e) = members(self, from_build).await?;
        let (new_n, new_e) = members(self, to_build).await?;
        let added_nodes: Vec<_> = new_n.difference(&old_n).cloned().collect();
        let removed_nodes: Vec<_> = old_n.difference(&new_n).cloned().collect();
        let added_edges: Vec<_> = new_e.difference(&old_e).cloned().collect();
        let removed_edges: Vec<_> = old_e.difference(&new_e).cloned().collect();
        Ok(serde_json::json!({
            "repo": repo, "from_build": from_build, "to_build": to_build,
            "added_nodes": added_nodes, "removed_nodes": removed_nodes,
            "added_edges": added_edges, "removed_edges": removed_edges,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaosbox_core::{diff_builds, SourceSpan};
    use chaosbox_store::Store as _;

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
        // Identity covers every threshold so raw decisions are reusable.
        let m1 = Materialization {
            accept_noul: 0.95,
            ..Default::default()
        };
        let m2 = Materialization::default();
        assert_ne!(m1.identity("x"), m2.identity("x"));
        let m3 = Materialization {
            accept_score: 2.0,
            ..Default::default()
        };
        assert_ne!(m3.identity("x"), m2.identity("x"));
        let m4 = Materialization {
            accept_confidence: 0.99,
            ..Default::default()
        };
        assert_ne!(m4.identity("x"), m2.identity("x"));
        let m5 = Materialization {
            abstain_confidence: 0.1,
            ..Default::default()
        };
        assert_ne!(m5.identity("x"), m2.identity("x"));
        assert!(m2.validate().is_ok());
        let bad = Materialization {
            abstain_confidence: 0.9,
            accept_confidence: 0.6,
            ..Default::default()
        };
        assert!(
            bad.validate().is_err(),
            "abstain floor above accept floor is incoherent"
        );
    }

    /// Responder with tunable Choice confidence for abstain tests.
    struct ConfResponder {
        confidence: f64,
    }

    #[async_trait::async_trait]
    impl Responder for ConfResponder {
        async fn respond(
            &mut self,
            _state: serde_json::Value,
            questions: BTreeMap<String, Question>,
        ) -> Result<SystemOneResponse, String> {
            let mut answers = BTreeMap::new();
            for id in questions.keys() {
                answers.insert(
                    id.clone(),
                    Answer::Choice(ChoiceAnswer {
                        choice: "accept".into(),
                        probabilities: BTreeMap::from([
                            ("accept".into(), 0.9),
                            ("reject".into(), 0.05),
                            ("none".into(), 0.05),
                        ]),
                        confidence: self.confidence,
                    }),
                );
            }
            Ok(SystemOneResponse {
                model: chaosbox_jev::JEV_MODEL_PINNED.into(),
                answers,
                usage: chaosbox_jev::Usage {
                    input_tokens: 1,
                    output_tokens: 0,
                },
            })
        }
    }

    /// Responder that fails every call (transport fault simulation).
    struct FailResponder;

    #[async_trait::async_trait]
    impl Responder for FailResponder {
        async fn respond(
            &mut self,
            _state: serde_json::Value,
            _questions: BTreeMap<String, Question>,
        ) -> Result<SystemOneResponse, String> {
            Err("transport down".into())
        }
    }

    fn one_candidate() -> (Candidate, BTreeMap<String, Entity>) {
        use chaosbox_core::{EntityKind, RelationType, SourceSpan};
        let span = SourceSpan::point("a.rs", 1, 1, 0);
        let from = Entity::new(EntityKind::Symbol, "r", "s", "a.rs", "a", "a", span.clone());
        let to = Entity::new(EntityKind::Symbol, "r", "s", "a.rs", "b", "b", span);
        let cand = Candidate {
            id: "cand:1".into(),
            rel_type: RelationType::Calls,
            from_entity: from.id.clone(),
            to_entity: to.id.clone(),
            reason: "structural".into(),
            state_excerpt: "a calls b".into(),
        };
        let entities = BTreeMap::from([(from.id.clone(), from), (to.id.clone(), to)]);
        (cand, entities)
    }

    #[tokio::test]
    async fn below_floor_confidence_abstains() {
        let (cand, entities) = one_candidate();
        let mat = Materialization::default();
        let mut store = MemoryStore::new();
        store
            .ensure_snapshot_files(
                "s",
                "r",
                &[chaosbox_core::SnapshotFile {
                    snapshot: "s".into(),
                    path: "a.rs".into(),
                    sha256: "abc".into(),
                    bytes: 3,
                }],
            )
            .await
            .unwrap();
        let mut low = ConfResponder { confidence: 0.1 };
        let decided = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut low,
            "jev-1.13.0",
            &mat,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(decided[0].1.outcome, DecisionOutcome::Abstained);
        assert_eq!(store.stats().decisions, 1, "abstentions persist");
        // Same inputs reuse the stored decision even when the responder would
        // now fail: no re-ask on a cache hit.
        let mut failing = FailResponder;
        let reused = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut failing,
            "jev-1.13.0",
            &mat,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(reused[0].1.outcome, DecisionOutcome::Abstained);
        // A fresh store re-asks: above the accept floor the answer is accepted.
        let mut fresh = MemoryStore::new();
        fresh
            .ensure_snapshot_files(
                "s",
                "r",
                &[chaosbox_core::SnapshotFile {
                    snapshot: "s".into(),
                    path: "a.rs".into(),
                    sha256: "abc".into(),
                    bytes: 3,
                }],
            )
            .await
            .unwrap();
        let mut high = ConfResponder { confidence: 0.95 };
        let decided = Pipeline::<MemoryStore>::decide(
            &[cand],
            &entities,
            &mut high,
            "jev-1.13.0",
            &mat,
            &mut fresh,
        )
        .await
        .unwrap();
        assert_eq!(decided[0].1.outcome, DecisionOutcome::Accepted);
    }

    #[tokio::test]
    async fn responder_faults_become_failed_decisions() {
        let (cand, entities) = one_candidate();
        let mat = Materialization::default();
        let mut store = MemoryStore::new();
        store
            .ensure_snapshot_files(
                "s",
                "r",
                &[chaosbox_core::SnapshotFile {
                    snapshot: "s".into(),
                    path: "a.rs".into(),
                    sha256: "abc".into(),
                    bytes: 3,
                }],
            )
            .await
            .unwrap();
        let mut failing = FailResponder;
        let decided = Pipeline::<MemoryStore>::decide(
            &[cand],
            &entities,
            &mut failing,
            "jev-1.13.0",
            &mat,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(decided.len(), 1, "batch continues past one fault");
        assert!(matches!(decided[0].1.outcome, DecisionOutcome::Failed(_)));
        // The fault text is never copied into evidence.
        assert!(!decided[0].2.text.contains("transport"));
        assert_eq!(store.stats().decisions, 1, "failures persist for retry");
    }

    #[tokio::test]
    async fn failed_refresh_preserves_last_good_build() {
        let (cand, entities) = one_candidate();
        let mat = Materialization::default();
        let snap = Snapshot {
            id: "s".into(),
            repo: "r".into(),
            files: vec![],
            contents: BTreeMap::new(),
        };
        let ext = Extraction {
            entities: entities.values().cloned().collect(),
            explicit_refs: vec![],
        };
        // First publish a good build on one pipeline/store.
        let mut pipe = Pipeline::<MemoryStore>::new();
        ensure_a_rs(&mut pipe.store).await;
        let mut accept = ConfResponder { confidence: 0.95 };
        let good = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut accept,
            "jev-1.13.0",
            &mat,
            &mut pipe.store,
        )
        .await
        .unwrap();
        let build = pipe
            .build_and_publish("r", &snap, &ext, &good, &mat, None)
            .await
            .unwrap();
        let active_before = pipe.store.active("r").map(|b| b.id);
        assert_eq!(active_before, Some(build.id.clone()));
        assert_eq!(pipe.generation, 1);
        // A failed refresh on the same repository must not publish: the
        // active build and generation stay exactly as-is. A new model
        // identity forces re-asking instead of reusing the cached accept.
        let mut failing = FailResponder;
        let bad = Pipeline::<MemoryStore>::decide(
            &[cand],
            &entities,
            &mut failing,
            "jev-9.9.9",
            &mat,
            &mut pipe.store,
        )
        .await
        .unwrap();
        assert!(matches!(bad[0].1.outcome, DecisionOutcome::Failed(_)));
        let err = pipe
            .build_and_publish("r", &snap, &ext, &bad, &mat, Some(build.id.clone()))
            .await
            .expect_err("failed batch must not publish");
        assert!(
            err.to_string().contains("refusing to publish"),
            "unexpected error: {err}"
        );
        assert_eq!(pipe.store.active("r").map(|b| b.id), active_before);
        assert_eq!(pipe.generation, 1, "rejected batch mints no generation");
        let counts = summarize_outcomes(&bad);
        assert_eq!(counts.get("failed").copied().unwrap_or(0), 1);
    }

    /// Ensure helper for the single-file `one_candidate` fixture.
    async fn ensure_a_rs(store: &mut MemoryStore) {
        store
            .ensure_snapshot_files(
                "s",
                "r",
                &[chaosbox_core::SnapshotFile {
                    snapshot: "s".into(),
                    path: "a.rs".into(),
                    sha256: "abc".into(),
                    bytes: 3,
                }],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cache_invalidates_per_axis() {
        let (cand, entities) = one_candidate();
        let mat = Materialization::default();
        let mut store = MemoryStore::new();
        ensure_a_rs(&mut store).await;
        // Baseline: accepted under jev-1.13.0 / rubric-v1.
        let mut accept = ConfResponder { confidence: 0.95 };
        let first = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut accept,
            "jev-1.13.0",
            &mat,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(first[0].1.outcome, DecisionOutcome::Accepted);
        // Model change invalidates: re-asked (low confidence now abstains),
        // and the stale row is replaced because the key differs.
        let mut low = ConfResponder { confidence: 0.1 };
        let second = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut low,
            "jev-9.9.9",
            &mat,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(second[0].1.outcome, DecisionOutcome::Abstained);
        // Rubric change invalidates the same way.
        let mat2 = Materialization {
            rubric_version: "rubric-v2".into(),
            ..Default::default()
        };
        let third = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut low,
            "jev-1.13.0",
            &mat2,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(third[0].1.outcome, DecisionOutcome::Abstained);
        // Catalog change (extra candidate) invalidates the whole run.
        let mut extra = cand.clone();
        extra.id = "cand:2".into();
        let fourth = Pipeline::<MemoryStore>::decide(
            &[cand.clone(), extra],
            &entities,
            &mut low,
            "jev-1.13.0",
            &mat,
            &mut store,
        )
        .await
        .unwrap();
        assert_eq!(fourth[0].1.outcome, DecisionOutcome::Abstained);
        // Threshold-only change keeps the key: the stored decision is reused
        // (materialization applies current thresholds later, not here).
        // Fresh store so earlier legs haven't replaced the row.
        let mut store2 = MemoryStore::new();
        ensure_a_rs(&mut store2).await;
        let mut accept2 = ConfResponder { confidence: 0.95 };
        let base = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut accept2,
            "jev-1.13.0",
            &mat,
            &mut store2,
        )
        .await
        .unwrap();
        assert_eq!(base[0].1.outcome, DecisionOutcome::Accepted);
        let mat3 = Materialization {
            accept_confidence: 0.99,
            ..Default::default()
        };
        let mut failing = FailResponder;
        let fifth = Pipeline::<MemoryStore>::decide(
            std::slice::from_ref(&cand),
            &entities,
            &mut failing,
            "jev-1.13.0",
            &mat3,
            &mut store2,
        )
        .await
        .unwrap();
        assert_eq!(
            fifth[0].1.outcome,
            DecisionOutcome::Accepted,
            "threshold change reuses raw decision"
        );
    }

    #[test]
    fn fresh_process_chains_off_the_live_build() {
        assert_eq!(chain_publication(None).unwrap(), (None, 0));
        assert_eq!(
            chain_publication(Some(("b1".into(), 3))).unwrap(),
            (Some("b1".into()), 3)
        );
        assert!(chain_publication(Some(("b1".into(), -1))).is_err());
    }

    #[tokio::test]
    async fn empty_decisions_publish_entities_only() {
        use chaosbox_core::EntityKind;
        use chaosbox_extract::FileVersion;
        let snap = Snapshot {
            id: "snap:x".into(),
            repo: "r".into(),
            files: vec![FileVersion {
                path: "a.rs".into(),
                sha256: "00".into(),
                bytes: 9,
            }],
            contents: BTreeMap::from([("a.rs".into(), "fn a() {}\n".into())]),
        };
        let ext = Extraction {
            entities: vec![Entity::new(
                EntityKind::Symbol,
                "r",
                &snap.id,
                "a.rs",
                "a",
                "a",
                SourceSpan::point("a.rs", 1, 1, 0),
            )],
            explicit_refs: vec![],
        };
        let mut pipe = Pipeline::<MemoryStore>::new();
        let build = pipe
            .build_and_publish("r", &snap, &ext, &[], &Materialization::default(), None)
            .await
            .unwrap();
        assert!(!build.nodes.is_empty(), "entities must publish");
        assert!(build.edges.is_empty(), "no decisions means no relations");
    }

    #[test]
    fn rel_filter_validation_is_loud() {
        assert_eq!(
            validate_rel_filter(Some(vec!["Calls".into()])).unwrap(),
            Some(vec!["calls".into()])
        );
        assert!(validate_rel_filter(None).unwrap().is_none());
        let err = validate_rel_filter(Some(vec!["frobnicate".into()])).unwrap_err();
        assert!(err.to_string().contains("valid:"), "{err}");
    }

    #[test]
    fn search_returns_sorted_top_n() {
        let mut b = GraphBuild::new("r", vec!["s".into()], 1);
        for name in ["zeta", "alpha", "gamma"] {
            b.add_node(Entity::new(
                chaosbox_core::EntityKind::Symbol,
                "r",
                "s",
                "a.rs",
                name,
                name,
                SourceSpan::point("a.rs", 1, 1, 0),
            ))
            .unwrap();
        }
        let hits = search(&b, "a", 2);
        let names: Vec<_> = hits.iter().map(|e| e.name.clone()).collect();
        // Sorted by qualified name first, then truncated: alpha, gamma.
        assert_eq!(names, vec!["alpha".to_owned(), "gamma".to_owned()]);
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

    #[tokio::test]
    async fn graph_reader_serves_fake_backend() {
        // The Phase 0 seam: the same reader code serves the in-memory fake.
        let seed = chaosbox_store::conformance_seed();
        let reader: GraphReader<chaosbox_store::MemoryReader> =
            GraphReader::pinned(seed.reader, "conf").await.unwrap();
        assert_eq!(reader.build_id, seed.builds.1);
        // Reads are scoped to the pinned build: only the second Alpha shows.
        let hits = reader.search("alpha", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity_id, seed.a2);
        assert!(reader.lookup(&seed.a1).await.unwrap().is_none());
        let path = reader.path(&seed.a2, &seed.b1, 4).await.unwrap();
        assert_eq!(path, None, "cross-build entities never connect");
        let explained = reader.explain(&seed.a2).await.unwrap();
        assert_eq!(explained["outgoing"], 1);
        // rel1 belongs to the first build, invisible from the pinned one.
        let ev = reader.evidence(&seed.rel1).await.unwrap();
        assert_eq!(ev["evidence"].as_array().unwrap().len(), 0);
        let v = reader.export().await.unwrap();
        assert_eq!(
            v["build_id"],
            serde_json::Value::String(seed.builds.1.clone())
        );
        let d = reader
            .diff("conf", &seed.builds.0, &seed.builds.1)
            .await
            .unwrap();
        assert!(!d["added_nodes"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn path_traversal_budget_is_explicit() {
        let seed = chaosbox_store::conformance_seed();
        let reader: GraphReader<chaosbox_store::MemoryReader> =
            GraphReader::pinned(seed.reader, "conf").await.unwrap();
        let gamma = &reader.search("Gamma", 10).await.unwrap()[0].entity_id;
        let ok = reader.path(&seed.a2, gamma, 4).await.unwrap();
        assert!(ok.is_some(), "a2 references Gamma in the pinned build");
        let err = reader
            .path_with_cap(&seed.a2, gamma, 4, 0)
            .await
            .expect_err("zero visit budget must fail, not hang");
        assert!(
            err.to_string().contains("budget exceeded"),
            "unexpected error: {err}"
        );
    }
}
