//! Semantic-row flush into `TypeDB`: decisions (with dedup re-read), evidence,
//! claims, and build rows.

use chaosbox_core::{
    Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild, Relation,
    evidence_class_name,
};
use chaosbox_store::StoreError;
use crate::common::{
    col_double_opt, col_string, decision_key, driver_error, file_version_id, link_id, now_millis,
    read_rows, span_id_of,
};
use crate::encode::{bool_lit, double_lit, int_lit, str_lit};

use super::TypeDbStore;

impl TypeDbStore {
    /// Conditional decision write: insert when absent, replace when the
    /// cache key changed or a recorded failure is retried, keep otherwise.
    /// One row per `(candidate, question)` key; retry-safe under retry and
    /// concurrent flush.
    pub(super) async fn flush_decision(&self, d: &Decision) -> Result<(), StoreError> {
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
        if existing.is_some() {
            let q = format!(
                "match $d isa decision, has decision-key {}; delete $d;",
                str_lit(&key)
            );
            self.write_one(&q).await.map_err(|e| driver_error(*e))?;
        }
        let outcome =
            serde_json::to_string(&d.outcome).map_err(|e| StoreError::Query(e.to_string()))?;
        let class = evidence_class_name(d.evidence_class);
        let mut owns = format!(
            "has decision-key {}, has decision-id {}, has candidate-id {}, has question-id {}, has outcome {}, has evidence-class {}, has model-requested {}, has model-returned {}, has cache-key {}",
            str_lit(&key),
            str_lit(&d.id),
            str_lit(&d.candidate_id),
            str_lit(&d.question_id),
            str_lit(&outcome),
            str_lit(&class),
            str_lit(&d.model_requested),
            str_lit(&d.model_returned),
            str_lit(&d.cache_key)
        );
        if let Some(c) = d.confidence {
            owns.push_str(", has confidence ");
            owns.push_str(&double_lit(c).map_err(|e| StoreError::Query(e.to_string()))?);
        }
        if let Some(p) = d.probability {
            owns.push_str(", has probability ");
            owns.push_str(&double_lit(p).map_err(|e| StoreError::Query(e.to_string()))?);
        }
        let q = format!("insert $d isa decision, {owns};");
        self.insert_ignoring_duplicates(&q).await
    }

    /// Read one decision row by key: (cache key, outcome json, full row).
    pub(super) async fn read_decision_row(
        &self,
        key: &str,
    ) -> Result<Option<(String, String, Decision)>, StoreError> {
        let q = format!(
            "match $d isa decision, has decision-key {}, has decision-id $id, has candidate-id $c, has question-id $q, has outcome $o, has evidence-class $e, has model-requested $mr, has model-returned $mrr, has cache-key $k; try {{ $d has confidence $cf; }}; try {{ $d has probability $p; }}; select $id, $c, $q, $o, $e, $mr, $mrr, $k, $cf, $p;",
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
            &["id", "c", "q", "o", "e", "mr", "mrr", "k", "cf", "p"],
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
