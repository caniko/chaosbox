//! Semantic-row flush into `TypeDB`: decisions (with dedup re-read), evidence,
//! claims, and build rows.

use chaosbox_core::{
    Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild, InferenceRecord,
    RawAnswer, Relation, evidence_class_name,
};
use chaosbox_store::StoreError;
use crate::common::{
    WRITE_TIMEOUT, col_double_opt, col_string, col_string_opt, decision_key, drain, driver_error,
    file_version_id, is_conflict, is_unique_violation, link_id, now_millis, read_rows, span_id_of,
};
use crate::encode::{bool_lit, double_lit, int_lit, str_lit};
use typedb_driver::{TransactionOptions, TransactionType};

use super::TypeDbStore;

impl TypeDbStore {
    /// Conditional decision write: insert when absent, replace when the
    /// cache key changed or a recorded failure is retried, keep otherwise.
    /// One row per `(candidate, question)` key; retry-safe under retry and
    /// concurrent flush.
    ///
    /// Atomicity: delete + insert execute in ONE write transaction, so a
    /// crash can never leave no decision. Concurrent replacements resolve
    /// via commit conflicts (`STC2`): the loser re-reads the winner and
    /// retries (bounded), converging instead of diverging or losing both.
    pub(super) async fn flush_decision(&self, d: &Decision) -> Result<(), StoreError> {
        for _ in 0..4 {
            let key = decision_key(&d.candidate_id, &d.question_id);
            let existing = self.read_decision_row(&key).await?;
            let replace = match &existing {
                None => true,
                Some((old_key, old_outcome, _)) => {
                    *old_key != d.cache_key || is_failed_outcome(old_outcome)
                }
            };
            if !replace {
                return Ok(());
            }
            match self.replace_decision_tx(&key, existing.is_some(), d).await {
                Ok(()) => return Ok(()),
                Err(StoreError::Invariant(msg)) if msg.contains("concurrent decision") => (),
                Err(e) => return Err(e),
            }
        }
        // Contention exhausted: if a winner exists, converge on it (caller
        // re-reads the winner); otherwise report.
        let key = decision_key(&d.candidate_id, &d.question_id);
        if self.read_decision_row(&key).await?.is_some() {
            return Ok(());
        }
        Err(StoreError::Invariant(
            "decision replacement contention exhausted".into(),
        ))
    }

    /// Atomic delete (when replacing) + insert in one write transaction.
    /// Returns `Invariant("concurrent decision...")` on commit conflicts or
    /// duplicate-key races so the caller retries against the fresh winner.
    async fn replace_decision_tx(
        &self,
        key: &str,
        had_existing: bool,
        d: &Decision,
    ) -> Result<(), StoreError> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("TypeDbStore disconnected".into()))?;
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        if had_existing {
            let del = format!(
                "match $d isa decision, has decision-key {}; delete $d;",
                str_lit(key)
            );
            match tx.query(&del).await {
                Ok(answer) => {
                    drain(answer).await.map_err(|e| {
                        if is_conflict(&e) || is_unique_violation(&e) {
                            StoreError::Invariant("concurrent decision write".into())
                        } else {
                            driver_error(*e)
                        }
                    })?;
                }
                Err(e) => {
                    if is_conflict(&e) || is_unique_violation(&e) {
                        return Err(StoreError::Invariant("concurrent decision write".into()));
                    }
                    return Err(driver_error(e));
                }
            }
        }
        let ins = Self::decision_insert_query(key, d)?;
        match tx.query(&ins).await {
            Ok(answer) => {
                drain(answer).await.map_err(|e| {
                    if is_conflict(&e) || is_unique_violation(&e) {
                        StoreError::Invariant("concurrent decision write".into())
                    } else {
                        driver_error(*e)
                    }
                })?;
            }
            Err(e) => {
                if is_conflict(&e) || is_unique_violation(&e) {
                    return Err(StoreError::Invariant("concurrent decision write".into()));
                }
                return Err(driver_error(e));
            }
        }
        match tx.commit().await {
            Ok(()) => Ok(()),
            Err(e) if is_conflict(&e) || is_unique_violation(&e) => {
                Err(StoreError::Invariant("concurrent decision write".into()))
            }
            Err(e) => Err(driver_error(e)),
        }
    }

    /// Build the decision insert query for one row (shared by the atomic
    /// replace path).
    fn decision_insert_query(key: &str, d: &Decision) -> Result<String, StoreError> {
        let outcome =
            serde_json::to_string(&d.outcome).map_err(|e| StoreError::Query(e.to_string()))?;
        let class = evidence_class_name(d.evidence_class);
        let mut owns = format!(
            "has decision-key {}, has decision-id {}, has candidate-id {}, has question-id {}, has outcome {}, has evidence-class {}, has model-requested {}, has model-returned {}, has cache-key {}",
            str_lit(key),
            str_lit(&d.id),
            str_lit(&d.candidate_id),
            str_lit(&d.question_id),
            str_lit(&outcome),
            str_lit(&class),
            str_lit(&d.model_requested),
            str_lit(&d.model_returned),
            str_lit(&d.cache_key)
        );
        if !d.reuse_key.is_empty() {
            owns.push_str(", has reuse-key ");
            owns.push_str(&str_lit(&d.reuse_key));
        }
        if let Some(raw) = &d.raw_answer {
            let raw_json =
                serde_json::to_string(raw).map_err(|e| StoreError::Query(e.to_string()))?;
            owns.push_str(", has raw-answer ");
            owns.push_str(&str_lit(&raw_json));
        }
        if let Some(c) = d.confidence {
            owns.push_str(", has confidence ");
            owns.push_str(&double_lit(c).map_err(|e| StoreError::Query(e.to_string()))?);
        }
        if let Some(p) = d.probability {
            owns.push_str(", has probability ");
            owns.push_str(&double_lit(p).map_err(|e| StoreError::Query(e.to_string()))?);
        }
        Ok(format!("insert $d isa decision, {owns};"))
    }

    /// Conditional inference write (issue #12): first write wins. The same
    /// reuse key always means the same inputs, so the first successful
    /// inference is the reproducible one; later writes never overwrite it.
    pub(super) async fn flush_inference(&self, rec: &InferenceRecord) -> Result<(), StoreError> {
        if self.read_inference_row(&rec.reuse_key).await?.is_some() {
            return Ok(());
        }
        let raw_json =
            serde_json::to_string(&rec.raw).map_err(|e| StoreError::Query(e.to_string()))?;
        let q = format!(
            "insert $i isa inference, has reuse-key {}, has raw-answer {}, has model-requested {}, has model-returned {};",
            str_lit(&rec.reuse_key),
            str_lit(&raw_json),
            str_lit(&rec.model_requested),
            str_lit(&rec.model_returned)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Read one reusable inference by reuse key.
    pub(super) async fn read_inference_row(
        &self,
        reuse_key: &str,
    ) -> Result<Option<InferenceRecord>, StoreError> {
        let q = format!(
            "match $i isa inference, has reuse-key {}, has raw-answer $r, has model-requested $mr, has model-returned $mrr; select $r, $mr, $mrr;",
            str_lit(reuse_key)
        );
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("TypeDbStore disconnected".into()))?;
        let rows = read_rows(driver, &self.config.database, &q, &["r", "mr", "mrr"]).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let raw_str = col_string(&row, "r")?;
        let raw: RawAnswer = serde_json::from_str(&raw_str)
            .map_err(|e| StoreError::Query(format!("bad raw-answer json: {e}")))?;
        Ok(Some(InferenceRecord {
            reuse_key: reuse_key.to_owned(),
            raw,
            model_requested: col_string(&row, "mr")?,
            model_returned: col_string(&row, "mrr")?,
        }))
    }

    /// Read one decision row by key: (cache key, outcome json, full row).
    /// Legacy rows without `reuse-key`/`raw-answer` read as empty/`None`
    /// (misses for relation-local reuse by construction).
    pub(super) async fn read_decision_row(
        &self,
        key: &str,
    ) -> Result<Option<(String, String, Decision)>, StoreError> {
        let q = format!(
            "match $d isa decision, has decision-key {}, has decision-id $id, has candidate-id $c, has question-id $q, has outcome $o, has evidence-class $e, has model-requested $mr, has model-returned $mrr, has cache-key $k; try {{ $d has reuse-key $rk; }}; try {{ $d has raw-answer $ra; }}; try {{ $d has confidence $cf; }}; try {{ $d has probability $p; }}; select $id, $c, $q, $o, $e, $mr, $mrr, $k, $rk, $ra, $cf, $p;",
            str_lit(key)
        );
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("TypeDbStore disconnected".into()))?;
        let rows = read_rows(
            driver,
            &self.config.database,
            &q,
            &[
                "id", "c", "q", "o", "e", "mr", "mrr", "k", "rk", "ra", "cf", "p",
            ],
        )
        .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let outcome_str = col_string(&row, "o")?;
        let outcome: DecisionOutcome = serde_json::from_str(&outcome_str)
            .map_err(|e| StoreError::Query(format!("bad outcome json: {e}")))?;
        let class_str = col_string(&row, "e")?;
        let evidence_class: EvidenceClass = serde_json::from_str(&format!("\"{class_str}\""))
            .map_err(|e| StoreError::Query(format!("bad evidence class: {e}")))?;
        let cache_key = col_string(&row, "k")?;
        let reuse_key = col_string_opt(&row, "rk").unwrap_or_default();
        let raw_answer = match col_string_opt(&row, "ra") {
            None => None,
            Some(s) => Some(
                serde_json::from_str::<RawAnswer>(&s)
                    .map_err(|e| StoreError::Query(format!("bad raw-answer json: {e}")))?,
            ),
        };
        let d = Decision {
            id: col_string(&row, "id")?,
            candidate_id: col_string(&row, "c")?,
            question_id: col_string(&row, "q")?,
            outcome,
            evidence_class,
            model_requested: col_string(&row, "mr")?,
            model_returned: col_string(&row, "mrr")?,
            confidence: col_double_opt(&row, "cf"),
            probability: col_double_opt(&row, "p"),
            cache_key: cache_key.clone(),
            reuse_key,
            raw_answer,
        };
        Ok(Some((cache_key, outcome_str, d)))
    }

    /// Insert one evidence row (idempotent).
    pub(super) async fn flush_evidence(&self, e: &Evidence) -> Result<(), StoreError> {
        let mut owns = format!(
            "has evidence-id {}, has class {}, has supports {}, has text {}, has file-version-id {}",
            str_lit(&e.id),
            str_lit(&evidence_class_name(e.class)),
            bool_lit(e.supports),
            str_lit(&e.text),
            str_lit(&file_version_id(&e.snapshot, &e.source_file_version))
        );
        if let Some(s) = &e.span {
            self.flush_span(s).await?;
            owns.push_str(", has span-id ");
            owns.push_str(&str_lit(&span_id_of(
                &s.file,
                s.start_line,
                s.start_col,
                s.end_line,
                s.end_col,
                s.byte_start,
                s.byte_end,
            )));
        }
        if let Some(producer) = &e.producer {
            owns.push_str(", has producer ");
            owns.push_str(&str_lit(producer));
        }
        let q = format!("insert $x isa evidence, {owns};");
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one claim with its supporting/contradicting links.
    pub(super) async fn flush_claim(&self, c: &Claim) -> Result<(), StoreError> {
        let q = format!(
            "insert $x isa claim, has claim-id {}, has relationship-id {}, has accepted {};",
            str_lit(&c.id),
            str_lit(&c.relation_id),
            bool_lit(c.accepted)
        );
        self.insert_ignoring_duplicates(&q).await?;
        for ev_id in &c.supporting {
            let q = format!(
                "match $c isa claim, has claim-id {}; $e isa evidence, has evidence-id {}; insert (claim: $c, evidence: $e) isa supporting, has supporting-id {};",
                str_lit(&c.id),
                str_lit(ev_id),
                str_lit(&link_id("sup", &c.id, ev_id))
            );
            self.insert_ignoring_duplicates(&q).await?;
        }
        for ev_id in &c.contradicting {
            let q = format!(
                "match $c isa claim, has claim-id {}; $e isa evidence, has evidence-id {}; insert (claim: $c, evidence: $e) isa contradicting, has contradicting-id {};",
                str_lit(&c.id),
                str_lit(ev_id),
                str_lit(&link_id("con", &c.id, ev_id))
            );
            self.insert_ignoring_duplicates(&q).await?;
        }
        Ok(())
    }

    /// Insert one build row plus its staged node/edge memberships.
    pub(super) async fn flush_build_rows(&self, build: &GraphBuild) -> Result<(), StoreError> {
        self.flush_repository(&build.repo).await?;
        for snapshot_id in &build.snapshot_ids {
            self.flush_snapshot(&build.repo, snapshot_id).await?;
        }
        let generation = i64::try_from(build.generation)
            .map_err(|_| StoreError::Invariant("generation overflows i64".into()))?;
        let mut owns = format!(
            "has build-id {}, has repo-name {}, has generation {}, has status \"staging\", has created {}",
            str_lit(&build.id),
            str_lit(&build.repo),
            int_lit(generation),
            int_lit(now_millis())
        );
        for snapshot_id in &build.snapshot_ids {
            owns.push_str(", has snapshot-id ");
            owns.push_str(&str_lit(snapshot_id));
        }
        if let Some(pred) = &build.predecessor {
            owns.push_str(", has predecessor ");
            owns.push_str(&str_lit(pred));
        }
        if let Some(coverage) = &build.coverage {
            let json =
                serde_json::to_string(coverage).map_err(|e| StoreError::Query(e.to_string()))?;
            owns.push_str(", has coverage-json ");
            owns.push_str(&str_lit(&json));
        }
        self.insert_ignoring_duplicates(&format!("insert $b isa graph-build, {owns};"))
            .await?;
        let mut nodes: Vec<&Entity> = build.nodes.values().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        for e in nodes {
            self.flush_entity(e).await?;
            self.flush_node_membership(&build.id, &e.id).await?;
        }
        let mut edges: Vec<&Relation> = build.edges.values().collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        for r in edges {
            self.flush_relationship(r).await?;
            self.flush_edge_membership(&build.id, &r.id).await?;
        }
        Ok(())
    }
}

/// A recorded `Failed` outcome is retryable even under the same cache key.
fn is_failed_outcome(outcome_json: &str) -> bool {
    outcome_json.contains("\"failed\"") || outcome_json.contains("\"Failed\"")
}
