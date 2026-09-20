//! Driver-backed `TypeDB` [`Store`](chaosbox_gel::Store) implementation.
//!
//! Write path mirrors [`chaosbox_gel::GelStore`]: every `put_*` validates
//! into an in-memory [`MemoryStore`](chaosbox_gel::MemoryStore) staging
//! area with identical semantics, and [`TypeDbStore::publish`] flushes the
//! staged rows idempotently before swinging the active-build pointer.
//!
//! Transaction discipline (all proven against `TypeDB` 3.13.0):
//! - One short write transaction per row insert; ingestion never holds a
//!   whole repository in one transaction.
//! - Duplicate `@key` inserts fail with the `@unique` violation (`CNT9`),
//!   which the flush treats as already-present; a concurrent rival's commit
//!   fails with an isolation conflict (`STC2`).
//! - Publication swings the pointer in a single write transaction that
//!   re-validates predecessor and generation live, so two publishers from
//!   the same generation cannot both succeed.
//! - Reads use read transactions; a mutation query in one fails server-side.
//! - Dropped transactions close without commit: staged-but-unflushed data
//!   never becomes visible because every consumer read pins the active
//!   build through the pointer.

use std::time::Duration;

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild,
    Relation, SnapshotFile, evidence_class_name, relation_type_name,
};
use chaosbox_gel::{GelError, MemoryStore, Store, StoreStats};
use futures::TryStreamExt;
use typedb_driver::{
    Address, Addresses, Credentials, DriverOptions, DriverTlsConfig, TransactionOptions,
    TransactionType, TypeDBDriver,
    answer::{ConceptRow, QueryAnswer},
};

use crate::common::{
    FLUSH_RETRIES, WRITE_TIMEOUT, col_double_opt, col_int, col_string, decision_key, driver_error,
    drain, edge_membership_id, file_version_id, fold, is_conflict, is_unique_violation, link_id,
    membership_id, now_millis, read_rows, span_id_of,
};
/// Connection config re-exported for backend constructors.
pub use crate::common::TypeDbConfig;
use crate::encode::{bool_lit, double_lit, int_lit, str_lit};

// TypeDbConfig and shared driver plumbing live in [`crate::common`].

/// TypeDB-backed [`Store`]: staging validation in memory, durable rows in
/// `TypeDB`, publication through the active-build pointer.
pub struct TypeDbStore {
    config: TypeDbConfig,
    driver: Option<TypeDBDriver>,
    staging: MemoryStore,
}

// Flush methods hold no client across awaits beyond one short transaction;
// the driver is `!Clone`, so each call borrows it for the call duration.
impl TypeDbStore {
    /// A disconnected store with empty staging; connects lazily.
    #[must_use]
    pub fn new(config: TypeDbConfig) -> Self {
        Self {
            config,
            driver: None,
            staging: MemoryStore::default(),
        }
    }

    /// Connect the driver and ensure the database exists.
    async fn ensure_connected(&mut self) -> Result<(), GelError> {
        if self.driver.is_none() {
            let address: Address = self
                .config
                .address
                .parse()
                .map_err(|e| GelError::Client(format!("bad address: {e}")))?;
            let driver = TypeDBDriver::new(
                Addresses::from_address(address),
                Credentials::new(&self.config.username, &self.config.password),
                DriverOptions::new(DriverTlsConfig::disabled()),
            )
            .await
            .map_err(driver_error)?;
            if !driver
                .databases()
                .contains(&self.config.database)
                .await
                .map_err(driver_error)?
            {
                driver
                    .databases()
                    .create(&self.config.database)
                    .await
                    .map_err(driver_error)?;
            }
            self.driver = Some(driver);
        }
        Ok(())
    }

    /// Apply the packaged schema. Idempotent: re-defining the identical
    /// schema commits cleanly, so `migrate` is safe to re-run.
    pub async fn migrate(&mut self) -> Result<(), GelError> {
        self.ensure_connected().await?;
        let driver = self.driver.as_ref().expect("connected above");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Schema,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        drain(tx.query(crate::SCHEMA_TQL).await.map_err(driver_error)?)
            .await
            .map_err(|e| driver_error(*e))?;
        tx.commit().await.map_err(driver_error)?;
        Ok(())
    }

    /// Run one write pipeline and commit. Duplicate `@key` inserts surface
    /// the caller's choice: map them to already-present or propagate. The
    /// driver error is boxed across the await boundary (`result_large_err`);
    /// classification happens on the box before mapping.
    async fn write_one(&self, query: &str) -> Result<(), Box<typedb_driver::Error>> {
        let driver = self.driver.as_ref().expect("connected before flush");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(Box::new)?;
        let answer = tx.query(query).await.map_err(Box::new)?;
        drain(answer).await?;
        tx.commit().await.map_err(Box::new)?;
        Ok(())
    }

    /// Insert-or-ignore: the `@unique` violation means a rival (or an
    /// earlier retry) already wrote this key.
    async fn insert_ignoring_duplicates(&self, query: &str) -> Result<(), GelError> {
        // Bounded transient retries on connection loss; conflicts and
        // constraint outcomes are decided, never retried.
        let mut attempt = 0;
        loop {
            match self.write_one(query).await {
                Ok(()) => return Ok(()),
                Err(e) if is_unique_violation(&e) => return Ok(()),
                Err(e)
                    if matches!(&*e, typedb_driver::Error::Connection(_))
                        && attempt < FLUSH_RETRIES =>
                {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
                Err(e) => return Err(driver_error(*e)),
            }
        }
    }

    /// Insert the repository row (idempotent).
    async fn flush_repository(&self, repo: &str) -> Result<(), GelError> {
        let q = format!("insert $x isa repository, has repo-name {};", str_lit(repo));
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one snapshot row (idempotent).
    async fn flush_snapshot(&self, repo: &str, snapshot_id: &str) -> Result<(), GelError> {
        let q = format!(
            "insert $x isa source-snapshot, has snapshot-id {}, has repo-name {}, has created {};",
            str_lit(snapshot_id),
            str_lit(repo),
            int_lit(now_millis())
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one file-version row (idempotent by deterministic key).
    async fn flush_file_version(
        &self,
        snapshot_id: &str,
        path: &str,
        sha: &str,
        bytes: u64,
    ) -> Result<(), GelError> {
        let bytes = i64::try_from(bytes)
            .map_err(|_| GelError::Invariant("file size overflows i64".into()))?;
        let q = format!(
            "insert $x isa file-version, has file-version-id {}, has snapshot-id {}, has path {}, has sha256 {}, has bytes {};",
            str_lit(&file_version_id(snapshot_id, path)),
            str_lit(snapshot_id),
            str_lit(path),
            str_lit(sha),
            int_lit(bytes)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one span row (idempotent by deterministic key).
    async fn flush_span(&self, span: &chaosbox_core::SourceSpan) -> Result<(), GelError> {
        let conv = |v: u32| int_lit(i64::from(v));
        let q = format!(
            "insert $x isa source-span, has span-id {}, has file {}, has start-line {}, has start-col {}, has end-line {}, has end-col {}, has byte-start {}, has byte-end {};",
            str_lit(&span_id_of(
                &span.file,
                span.start_line,
                span.start_col,
                span.end_line,
                span.end_col,
                span.byte_start,
                span.byte_end
            )),
            str_lit(&span.file),
            conv(span.start_line),
            conv(span.start_col),
            conv(span.end_line),
            conv(span.end_col),
            conv(span.byte_start),
            conv(span.byte_end)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one entity row with its span (idempotent; first write wins,
    /// matching the Gel upsert that keeps the existing row on conflict).
    async fn flush_entity(&self, e: &Entity) -> Result<(), GelError> {
        self.flush_span(&e.span).await?;
        let kind = serde_json::to_value(&e.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{:?}", e.kind));
        let q = format!(
            "insert $x isa code-entity, has entity-id {}, has kind {}, has repo-name {}, has snapshot-id {}, has file {}, has name {}, has name-fold {}, has qualified-name {}, has qualified-name-fold {}, has span-id {};",
            str_lit(&e.id),
            str_lit(&kind),
            str_lit(&e.repo),
            str_lit(&e.snapshot),
            str_lit(&e.file),
            str_lit(&e.name),
            str_lit(&fold(&e.name)),
            str_lit(&e.qualified_name),
            str_lit(&fold(&e.qualified_name)),
            str_lit(&span_id_of(
                &e.span.file,
                e.span.start_line,
                e.span.start_col,
                e.span.end_line,
                e.span.end_col,
                e.span.byte_start,
                e.span.byte_end
            ))
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one relationship with typed endpoint roles (idempotent).
    async fn flush_relationship(&self, r: &Relation) -> Result<(), GelError> {
        let rel_type = relation_type_name(&r.rel_type);
        let scope = serde_json::to_value(&r.scope)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{:?}", r.scope));
        let q = format!(
            "match $f isa code-entity, has entity-id {}; $t isa code-entity, has entity-id {}; insert (from-entity: $f, to-entity: $t) isa relationship, has rel-id {}, has rel-type {}, has scope {};",
            str_lit(&r.from),
            str_lit(&r.to),
            str_lit(&r.id),
            str_lit(&rel_type),
            str_lit(&scope)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one node-membership relation (idempotent by deterministic key).
    async fn flush_node_membership(&self, build_id: &str, entity_id: &str) -> Result<(), GelError> {
        let q = format!(
            "match $b isa graph-build, has build-id {}; $e isa code-entity, has entity-id {}; insert (build: $b, member: $e) isa node-membership, has membership-id {};",
            str_lit(build_id),
            str_lit(entity_id),
            str_lit(&membership_id(build_id, entity_id))
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one edge-membership relation (idempotent by deterministic key).
    async fn flush_edge_membership(&self, build_id: &str, rel_id: &str) -> Result<(), GelError> {
        let q = format!(
            "match $b isa graph-build, has build-id {}; $rel isa relationship, has rel-id {}; insert (build: $b, edge: $rel) isa edge-membership, has edge-membership-id {};",
            str_lit(build_id),
            str_lit(rel_id),
            str_lit(&edge_membership_id(build_id, rel_id))
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one extraction-run row (idempotent).
    async fn flush_run(&self, run_id: &str, repo: &str, snapshot_id: &str) -> Result<(), GelError> {
        let q = format!(
            "insert $x isa extraction-run, has run-id {}, has repo-name {}, has snapshot-id {}, has created {};",
            str_lit(run_id),
            str_lit(repo),
            str_lit(snapshot_id),
            int_lit(now_millis())
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one candidate-set row (idempotent).
    async fn flush_set(
        &self,
        set_id: &str,
        run_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), GelError> {
        let q = format!(
            "insert $x isa candidate-set, has set-id {}, has run-id {}, has catalog-digest {}, has rubric-version {};",
            str_lit(set_id),
            str_lit(run_id),
            str_lit(catalog_digest),
            str_lit(rubric_version)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one candidate row (idempotent).
    async fn flush_candidate(&self, set_id: &str, c: &Candidate) -> Result<(), GelError> {
        let q = format!(
            "insert $x isa candidate, has candidate-id {}, has set-id {}, has rel-type {}, has from-entity-id {}, has to-entity-id {}, has reason {}, has state-excerpt {};",
            str_lit(&c.id),
            str_lit(set_id),
            str_lit(&relation_type_name(&c.rel_type)),
            str_lit(&c.from_entity),
            str_lit(&c.to_entity),
            str_lit(&c.reason),
            str_lit(&c.state_excerpt)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Conditional decision write: insert when absent, replace when the
    /// cache key changed or a recorded failure is retried, keep otherwise.
    /// Mirrors the Gel `UPSERT_DECISION` row contract.
    async fn flush_decision(&self, d: &Decision) -> Result<(), GelError> {
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
            serde_json::to_string(&d.outcome).map_err(|e| GelError::Query(e.to_string()))?;
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
            owns.push_str(&double_lit(c).map_err(|e| GelError::Query(e.to_string()))?);
        }
        if let Some(p) = d.probability {
            owns.push_str(", has probability ");
            owns.push_str(&double_lit(p).map_err(|e| GelError::Query(e.to_string()))?);
        }
        let q = format!("insert $d isa decision, {owns};");
        self.insert_ignoring_duplicates(&q).await
    }

    /// Read one decision row by key: (cache key, outcome json, full row).
    async fn read_decision_row(
        &self,
        key: &str,
    ) -> Result<Option<(String, String, Decision)>, GelError> {
        let q = format!(
            "match $d isa decision, has decision-key {}, has decision-id $id, has candidate-id $c, has question-id $q, has outcome $o, has evidence-class $e, has model-requested $mr, has model-returned $mrr, has cache-key $k; try {{ $d has confidence $cf; }}; try {{ $d has probability $p; }}; select $id, $c, $q, $o, $e, $mr, $mrr, $k, $cf, $p;",
            str_lit(key)
        );
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| GelError::Client("TypeDbStore disconnected".into()))?;
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
            .map_err(|e| GelError::Query(format!("bad outcome json: {e}")))?;
        let class_str = col_string(&row, "e")?;
        let evidence_class: EvidenceClass = serde_json::from_str(&format!("\"{class_str}\""))
            .map_err(|e| GelError::Query(format!("bad evidence class: {e}")))?;
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
    async fn flush_evidence(&self, e: &Evidence) -> Result<(), GelError> {
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
    async fn flush_claim(&self, c: &Claim) -> Result<(), GelError> {
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
    async fn flush_build_rows(&self, build: &GraphBuild) -> Result<(), GelError> {
        self.flush_repository(&build.repo).await?;
        for snapshot_id in &build.snapshot_ids {
            self.flush_snapshot(&build.repo, snapshot_id).await?;
        }
        let generation = i64::try_from(build.generation)
            .map_err(|_| GelError::Invariant("generation overflows i64".into()))?;
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

    /// Flush staged runs, sets, candidates, decisions, evidence, and claims
    /// in dependency order (all idempotent; safe to retry after a crash).
    async fn flush_chain(&self) -> Result<(), GelError> {
        let staged = self.staging.export_staged();
        for (run_id, (repo, snapshot_id)) in &staged.runs {
            self.flush_repository(repo).await?;
            self.flush_snapshot(repo, snapshot_id).await?;
            self.flush_run(run_id, repo, snapshot_id).await?;
        }
        for (set_id, (run_id, catalog, rubric)) in &staged.sets {
            self.flush_set(set_id, run_id, catalog, rubric).await?;
        }
        for (_, (set_id, cand)) in &staged.candidates {
            self.flush_candidate(set_id, cand).await?;
        }
        for d in &staged.decisions {
            self.flush_decision(d).await?;
        }
        for ((snapshot_id, path), (sha, bytes)) in &staged.files {
            self.flush_file_version(snapshot_id, path, sha, *bytes)
                .await?;
        }
        for e in &staged.evidence {
            self.flush_evidence(e).await?;
        }
        for c in &staged.claims {
            self.flush_claim(c).await?;
        }
        Ok(())
    }

    /// Live pointer read: (build id, generation, status) for a repository.
    async fn live_pointer(&self, repo: &str) -> Result<Option<(String, i64, String)>, GelError> {
        let q = format!(
            "match $p isa active-pointer, has repo-name {}, has build-id $b; $g isa graph-build, has build-id $b, has generation $gen, has status $st; select $b, $gen, $st;",
            str_lit(repo)
        );
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| GelError::Client("TypeDbStore disconnected".into()))?;
        let rows = read_rows(driver, &self.config.database, &q, &["b", "gen", "st"]).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let gen = col_int(&row, "gen")?;
        Ok(Some((col_string(&row, "b")?, gen, col_string(&row, "st")?)))
    }

    /// Guarded pointer swing in ONE write transaction: re-validate the
    /// predecessor and generation against live state, then move the pointer
    /// and mark the build active together. A rival's concurrent swing makes
    /// this commit fail with an isolation conflict.
    async fn swing(
        &self,
        repo: &str,
        build_id: &str,
        expected: Option<(String, i64)>,
        generation: i64,
    ) -> Result<(), GelError> {
        let driver = self.driver.as_ref().expect("connected before publish");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        // Live re-validation inside the swing transaction: the match binds
        // only when the pointer still holds the expected predecessor at a
        // generation the new build exceeds.
        let guard = match &expected {
            None => format!(
                "match not {{ $p isa active-pointer, has repo-name {}; }}; $b isa graph-build, has build-id {}, has status \"staging\"; select $b;",
                str_lit(repo),
                str_lit(build_id)
            ),
            Some((pred, pred_gen)) => format!(
                "match $p isa active-pointer, has repo-name {}, has build-id {}; $c isa graph-build, has build-id {}, has generation {}; $b isa graph-build, has build-id {}, has generation {}, has status \"staging\"; select $b;",
                str_lit(repo),
                str_lit(pred),
                str_lit(pred),
                int_lit(*pred_gen),
                str_lit(build_id),
                int_lit(generation)
            ),
        };
        let guard_rows: Vec<ConceptRow> = match tx.query(&guard).await.map_err(driver_error)? {
            QueryAnswer::ConceptRowStream(_, stream) => {
                stream.try_collect().await.map_err(driver_error)?
            }
            other => {
                return Err(GelError::Query(format!(
                    "guard expected rows, got {other:?}"
                )));
            }
        };
        if guard_rows.is_empty() {
            return Err(GelError::Invariant(
                "concurrent publisher moved the pointer; build staged but not activated".into(),
            ));
        }
        if expected.is_none() {
            let q = format!(
                "insert $p isa active-pointer, has repo-name {}, has build-id {}, has updated {};",
                str_lit(repo),
                str_lit(build_id),
                int_lit(now_millis())
            );
            drain(tx.query(&q).await.map_err(driver_error)?)
                .await
                .map_err(|e| driver_error(*e))?;
        } else {
            let q = format!(
                "match $p isa active-pointer, has repo-name {}; update $p has build-id {}, has updated {};",
                str_lit(repo),
                str_lit(build_id),
                int_lit(now_millis())
            );
            drain(tx.query(&q).await.map_err(driver_error)?)
                .await
                .map_err(|e| driver_error(*e))?;
        }
        let q = format!(
            "match $b isa graph-build, has build-id {}; update $b has status \"active\";",
            str_lit(build_id)
        );
        drain(tx.query(&q).await.map_err(driver_error)?)
            .await
            .map_err(|e| driver_error(*e))?;
        match tx.commit().await {
            Ok(()) => Ok(()),
            Err(e) if is_conflict(&e) => Err(GelError::Invariant(
                "concurrent publisher moved the pointer; build staged but not activated".into(),
            )),
            Err(e) => {
                // Uncertain commit: reconcile by reading durable state before
                // reporting. If the pointer holds our build, the swing won.
                match self.live_pointer(repo).await {
                    Ok(Some((active, _, _))) if active == build_id => Ok(()),
                    _ => Err(driver_error(e)),
                }
            }
        }
    }
}

/// A recorded `Failed` outcome is retryable even under the same cache key.
fn is_failed_outcome(outcome_json: &str) -> bool {
    outcome_json.contains("\"failed\"") || outcome_json.contains("\"Failed\"")
}

impl Default for TypeDbStore {
    fn default() -> Self {
        Self::new(TypeDbConfig {
            address: "127.0.0.1:1729".to_owned(),
            username: "admin".to_owned(),
            password: String::new(),
            database: "chaosbox".to_owned(),
        })
    }
}

#[async_trait::async_trait]
impl Store for TypeDbStore {
    async fn ensure_snapshot_files(
        &mut self,
        snapshot_id: &str,
        repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), GelError> {
        self.staging
            .ensure_snapshot_files(snapshot_id, repo, files)
            .await
    }

    async fn ensure_run(
        &mut self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
        set_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), GelError> {
        self.staging
            .ensure_run(
                run_id,
                repo,
                snapshot_id,
                set_id,
                catalog_digest,
                rubric_version,
            )
            .await
    }

    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), GelError> {
        self.staging.put_candidate(set_id, c).await
    }

    async fn put_entity(&mut self, e: Entity) -> Result<(), GelError> {
        self.staging.put_entity(e).await
    }

    async fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), GelError> {
        self.staging.put_relation(r, build_id).await
    }

    async fn put_decision(&mut self, d: Decision) -> Result<(), GelError> {
        // Non-finite floats have no TypeQL form: reject at the boundary
        // instead of failing mid-flush.
        if let Some(c) = d.confidence {
            double_lit(c).map_err(|e| GelError::Invariant(e.to_string()))?;
        }
        if let Some(p) = d.probability {
            double_lit(p).map_err(|e| GelError::Invariant(e.to_string()))?;
        }
        self.staging.put_decision(d).await
    }

    async fn put_evidence(&mut self, e: Evidence) -> Result<(), GelError> {
        self.staging.put_evidence(e).await
    }

    async fn put_claim(&mut self, c: Claim) -> Result<(), GelError> {
        self.staging.put_claim(c).await
    }

    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, GelError> {
        if let Some(d) = self
            .staging
            .find_decision(candidate_id, question_id)
            .await?
        {
            return Ok(Some(d));
        }
        if self.driver.is_none() {
            return Ok(None);
        }
        let key = decision_key(candidate_id, question_id);
        Ok(self.read_decision_row(&key).await?.map(|(_, _, d)| d))
    }

    async fn publish(
        &mut self,
        build: GraphBuild,
        expected_predecessor: Option<String>,
    ) -> Result<(), GelError> {
        // 1. Live-pointer guard before writing anything (mirrors GelStore:
        // idempotent retry returns early, stale predecessors fail here).
        self.ensure_connected().await?;
        let live = self.live_pointer(&build.repo).await?;
        let pred_gen = match (&live, &expected_predecessor) {
            (None, None) => None,
            (Some((active_id, gen, _)), _) if *active_id == build.id => return Ok(()),
            (Some((active_id, gen, _)), Some(pred))
                if *pred == *active_id
                    && i64::try_from(build.generation)
                        .map_err(|_| GelError::Invariant("generation overflows i64".into()))?
                        > *gen =>
            {
                Some((pred.clone(), *gen))
            }
            (Some((active_id, gen, _)), None) => {
                return Err(GelError::Invariant(format!(
                    "predecessor mismatch: expected None, live active is {active_id} (gen {gen})"
                )));
            }
            (Some((active_id, gen, _)), Some(pred)) => {
                let build_gen = i64::try_from(build.generation)
                    .map_err(|_| GelError::Invariant("generation overflows i64".into()))?;
                if *pred == *active_id && build_gen <= *gen {
                    return Err(GelError::Invariant(
                        "older worker cannot replace newer build".into(),
                    ));
                }
                return Err(GelError::Invariant(format!(
                    "predecessor mismatch: expected {pred:?}, live active is {active_id} (gen {gen})"
                )));
            }
            (None, Some(pred)) => {
                return Err(GelError::Invariant(format!(
                    "expected predecessor {pred} but no live active build"
                )));
            }
        };
        // 2. Local invariant validation (cross-build edges, generations).
        // The staging predecessor check is per-process: a fresh process has
        // empty staging, so its predecessor is validated against live state
        // (step 1) and the swing (step 4) instead of failing here. A
        // same-process re-publish still goes through the staging check.
        let staging_pred = if self.staging.active(&build.repo).is_none() {
            None
        } else {
            expected_predecessor.clone()
        };
        self.staging.publish(build.clone(), staging_pred).await?;
        // 3. Idempotent flush; safe to retry after a crash mid-flush.
        self.flush_build_rows(&build).await?;
        self.flush_chain().await?;
        // 4. Guarded swing last: a concurrent publisher wins instead of
        // being overwritten, and the last good build stays active. NOTE: a
        // failed swing leaves this instance's staging ahead of live; retry
        // publication with a fresh store.
        let generation = i64::try_from(build.generation)
            .map_err(|_| GelError::Invariant("generation overflows i64".into()))?;
        self.swing(&build.repo, &build.id, pred_gen, generation)
            .await
    }

    fn active(&self, repo: &str) -> Option<GraphBuild> {
        self.staging.active(repo)
    }

    fn get(&self, build_id: &str) -> Option<GraphBuild> {
        self.staging.get(build_id)
    }

    fn stats(&self) -> StoreStats {
        self.staging.stats()
    }
}
