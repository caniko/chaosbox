//! Pipeline state, the uncached-decision preflight, decisions, and publication.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Full pipeline state held by the operator commands.
/// Generic over [`chaosbox_store::Store`] with [`MemoryStore`] as the default.
pub struct Pipeline<S = MemoryStore> {
    /// Backing store (decisions, evidence, builds, active pointer).
    pub store: S,
    /// Local generation counter for builds published through this pipeline.
    pub generation: u64,
}

/// Count candidates whose reusable inference is missing or invalid: no
/// validated hit under the relation-local reuse key. Each such candidate
/// costs exactly one live Jev request, so operators can check
/// `uncached <= max_requests` **before** any spend — the budget preflight
/// in `run --live-jev`.
///
/// Shares [`crate::reuse::resolve_reuse`] verbatim with [`Pipeline::decide`]
/// and [`crate::decide_cached`]: same reuse inputs, same raw validation,
/// same model provenance, same Failed-retry rule. Failed attempts are never
/// stored as reusable inferences, so they always count as uncached
/// (retries re-ask). Legacy decisions without raw data are misses by
/// construction (nothing stored under their key): they re-ask once to
/// populate the inference.
pub async fn uncached_decisions<S: chaosbox_store::Store>(
    candidates: &[Candidate],
    entities: &BTreeMap<String, Entity>,
    snapshot: &Snapshot,
    model_requested: &str,
    mat: &Materialization,
    policy: &chaosbox_core::EffectivePolicy,
    store: &S,
) -> Result<usize, PipelineError> {
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
    let mut uncached = 0usize;
    for cand in candidates {
        let from = entities
            .get(&cand.from_entity)
            .ok_or_else(|| PipelineError::Validation("missing from".into()))?;
        let to = entities
            .get(&cand.to_entity)
            .ok_or_else(|| PipelineError::Validation("missing to".into()))?;
        let attempt =
            crate::reuse::resolve_reuse(cand, from, to, &snapshot.id, &ctx, mat, &catalog, store)
                .await?;
        if attempt.hit.is_none() {
            uncached += 1;
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
    /// Scope comes from the effective policy so the snapshot identity and
    /// the decision cache identity can never diverge on what was indexed.
    pub fn snapshot_extract(
        repo: &str,
        root: &Path,
        max_candidates: usize,
        policy: &chaosbox_core::EffectivePolicy,
    ) -> Result<(Snapshot, Extraction, CandidateCatalog), PipelineError> {
        let snap = Snapshot::capture_scoped(repo, root, &policy.scope)
            .map_err(|e| PipelineError::Extract(e.to_string()))?;
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
    ///
    /// Reuse (issue #12): the shared [`crate::reuse::resolve_reuse`]
    /// authorizes cross-snapshot reuse. A validated stored inference
    /// rematerializes under current thresholds without spending; misses call
    /// the responder once, persist the reusable inference authoritatively,
    /// then materialize the stored winner (concurrent racers converge).
    /// Failed attempts never store an inference (retries always re-ask).
    /// Threshold changes replace stored decisions/evidence via the
    /// materialization digest in their identity — returned and persisted
    /// rows agree.
    // Long decision pipeline; splitting stages apart is the owning
    // session's refactor. Allowed to keep CI unblocked.
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    pub async fn decide(
        candidates: &[Candidate],
        entities: &BTreeMap<String, Entity>,
        snapshot: &Snapshot,
        responder: &mut impl Responder,
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
            let attempt = crate::reuse::resolve_reuse(
                cand,
                from,
                to,
                &snapshot.id,
                &ctx,
                mat,
                &catalog,
                &*store,
            )
            .await?;
            let rkey = attempt.reuse_key.clone();
            let qid = attempt.qid.clone();
            let audit_key = attempt.audit_cache_key.clone();
            let mat_digest = attempt.mat_digest.clone();
            // Cross-snapshot reuse: rematerialize the validated stored raw
            // under current thresholds; evidence rebinds to current ids.
            // Identity carries reuse key + materialization digest, so a
            // threshold or policy change replaces the stored row instead of
            // leaving it stale, and evidence ids (content-addressed from
            // the decision) never collide across materializations.
            if let Some(hit) = attempt.hit {
                let decision = Decision {
                    id: decision_id_for(&cand.id, &qid, model_requested, &rkey, &mat_digest),
                    candidate_id: cand.id.clone(),
                    question_id: qid.clone(),
                    outcome: hit.outcome.clone(),
                    evidence_class: hit.evidence_class,
                    model_requested: model_requested.to_owned(),
                    model_returned: hit.inference.model_returned.clone(),
                    confidence: hit.confidence,
                    probability: hit.probability,
                    cache_key: audit_key,
                    reuse_key: rkey,
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
                    .put_decision(decision.clone())
                    .await
                    .map_err(|e| PipelineError::Store(e.to_string()))?;
                store
                    .put_evidence(ev.clone())
                    .await
                    .map_err(|e| PipelineError::Store(e.to_string()))?;
                out.push((cand.clone(), decision, ev));
                continue;
            }
            let questions = attempt.questions.clone();
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
            // No inference is stored: failures always re-ask.
            let Ok(r) = responder.respond(state, questions.clone()).await else {
                let decision = Decision {
                    id: decision_id_for(&cand.id, "failed", model_requested, &rkey, &mat_digest),
                    candidate_id: cand.id.clone(),
                    question_id: qid.clone(),
                    outcome: DecisionOutcome::Failed("responder fault".into()),
                    evidence_class: EvidenceClass::Ambiguous,
                    model_requested: model_requested.to_owned(),
                    model_returned: String::new(),
                    confidence: None,
                    probability: None,
                    cache_key: audit_key,
                    reuse_key: rkey,
                    raw_answer: None,
                };
                let ev = assemble_evidence(
                    &decision,
                    false,
                    "decision attempt failed; see attempt accounting".into(),
                    None,
                    &snapshot.id,
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
            // Validate + reconcile per question. The present pipeline asks
            // exactly one semantic question per candidate; multi-question
            // reuse needs per-question raw records (follow-up).
            if questions.len() != 1 || resp.answers.len() != 1 {
                return Err(PipelineError::Validation(
                    "expected exactly one answer for one question".into(),
                ));
            }
            let valid = crate::reuse::valid_options_for(&qid);
            chaosbox_jev::validate_response_for_model(&resp, &questions, &valid, model_requested)
                .map_err(|e| PipelineError::Validation(e.to_string()))?;
            let (answered_qid, ans) = resp.answers.iter().next().expect("checked above");
            let raw = chaosbox_jev::raw_from_answer(ans);
            // Durable-first + authoritative winner: persist the reusable
            // inference, then re-read the server winner (first-write-wins)
            // and validate it through the SAME shared validator cached hits
            // use. Concurrent racers converge on the winner; a poisoned or
            // mismatched winner fails closed instead of materializing.
            let submitted = InferenceRecord {
                reuse_key: rkey.clone(),
                raw: raw.clone(),
                model_requested: model_requested.to_owned(),
                model_returned: resp.model.clone(),
            };
            store
                .put_inference(submitted)
                .await
                .map_err(|e| PipelineError::Store(e.to_string()))?;
            let winner = store
                .find_inference(&rkey)
                .await
                .map_err(|e| PipelineError::Store(e.to_string()))?
                .ok_or_else(|| PipelineError::Store("inference missing after put".into()))?;
            // Shared validation (key, provenance, current-question
            // semantics) — never materialize an unvalidated winner.
            let _checked = crate::reuse::validate_stored_inference(
                &winner,
                &rkey,
                model_requested,
                &questions,
                &qid,
            )?;
            let (outcome, class, conf, prob) = materialize_raw(&winner.raw, mat, &cand.reason)?;
            let decision = Decision {
                id: decision_id_for(&cand.id, answered_qid, model_requested, &rkey, &mat_digest),
                candidate_id: cand.id.clone(),
                question_id: answered_qid.clone(),
                outcome: outcome.clone(),
                evidence_class: class,
                model_requested: model_requested.to_owned(),
                model_returned: winner.model_returned.clone(),
                confidence: conf,
                probability: prob,
                cache_key: audit_key,
                reuse_key: rkey,
                raw_answer: Some(winner.raw.clone()),
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
                &snapshot.id,
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
        for (_, decision, _) in decided {
            chaosbox_jev::validate_model_identity(
                &decision.model_requested,
                &decision.model_returned,
            )
            .map_err(|e| PipelineError::Validation(e.to_string()))?;
        }
        self.generation += 1;
        let mut build = GraphBuild::new(repo, vec![snapshot.id.clone()], self.generation);
        if let Some(compiler) = &extraction.compiler {
            build.id = chaosbox_core::deterministic_id("build", &[&build.id, &compiler.context.id]);
        }
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
        crate::structural::publish_facts(
            &mut self.store,
            &mut build,
            snapshot,
            extraction,
            &entities,
        )
        .await?;
        let direct_relations = build.edges.len();
        self.materialize_decisions(&mut build, decided, mat, &entities)
            .await?;
        build.coverage = Some(chaosbox_core::coverage::BuildCoverage {
            files: extraction.coverage.clone(),
            structural_relations: direct_relations
                - extraction
                    .compiler
                    .as_ref()
                    .map_or(0, chaosbox_core::compiler::CompilerCoverage::relations),
            decision_relations: build.edges.len() - direct_relations,
            compiler: extraction.compiler.clone(),
        });
        // Invariant: published edges refer to same-build members (enforced by add_edge).
        self.store
            .publish(build.clone(), expected_predecessor)
            .await
            .map_err(|e| PipelineError::Store(e.to_string()))?;
        Ok(build)
    }

    async fn materialize_decisions(
        &mut self,
        build: &mut GraphBuild,
        decided: &[(Candidate, Decision, Evidence)],
        mat: &Materialization,
        entities: &BTreeMap<String, Entity>,
    ) -> Result<(), PipelineError> {
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
        Ok(())
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
