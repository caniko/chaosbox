//! Driver-backed TypeDB [`Store`](chaosbox_gel::Store) implementation.
//!
//! Write path mirrors [`chaosbox_gel::GelStore`]: every `put_*` validates
//! into an in-memory [`MemoryStore`](chaosbox_gel::MemoryStore) staging
//! area with identical semantics, and [`TypeDbStore::publish`] flushes the
//! staged rows idempotently before swinging the active-build pointer.
//!
//! Transaction discipline (all proven against TypeDB 3.13.0):
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

use std::collections::BTreeMap;
use std::time::Duration;

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild,
    Relation, SnapshotFile, deterministic_id, evidence_class_name, relation_type_name,
};
use chaosbox_gel::{GelError, MemoryStore, Store, StoreStats};
use futures::TryStreamExt;
use typedb_driver::{
    Address, Addresses, Credentials, DriverOptions, DriverTlsConfig, TransactionOptions,
    TransactionType, TypeDBDriver,
    answer::{ConceptRow, QueryAnswer},
    concept::{Concept, Value},
};

use crate::encode::{bool_lit, double_lit, int_lit, str_lit};

/// Connection and database selection for one [`TypeDbStore`].
#[derive(Clone, Debug)]
pub struct TypeDbConfig {
    /// Server address, e.g. `127.0.0.1:1729`.
    pub address: String,
    /// Application username (never the bootstrap admin in production).
    pub username: String,
    /// Application password (delivered via credential file upstream).
    pub password: String,
    /// TypeDB database name holding the Chaosbox schema and rows.
    pub database: String,
}

/// Bounded transaction lifetimes: staging writes are small, the pointer
/// swing must not hang a publisher forever.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Reads are bounded so a wedged server fails a consumer request instead of
/// hanging it; callers apply their own tighter deadlines on top.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Transient (connection-level) flush retries; conflicts are never retried
/// blindly because a retry could mask a lost publication race.
const FLUSH_RETRIES: usize = 3;

/// True for the `@unique` violation a duplicate `@key` insert raises.
/// Proven shape: code `CNT9`.
fn is_unique_violation(e: &typedb_driver::Error) -> bool {
    e.code() == "CNT9"
}

/// True for a commit-time isolation conflict between concurrent writers.
/// Proven shape: code `STC2`.
fn is_conflict(e: &typedb_driver::Error) -> bool {
    e.code() == "STC2"
}

/// Classify a driver failure for the [`Store`] surface.
fn driver_error(e: typedb_driver::Error) -> GelError {
    match &e {
        typedb_driver::Error::Connection(_) => GelError::Client(e.to_string()),
        _ => GelError::Query(format!("[{}] {e}", e.code())),
    }
}

/// Current time as integer epoch millis for `created`/`updated` attributes.
fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(i64::MAX)
}

/// Deterministic key for a file version: replaces Gel's exclusive
/// `(snapshot, path)` constraint with a content-derived `@key`.
fn file_version_id(snapshot: &str, path: &str) -> String {
    deterministic_id("fv", &[snapshot, path])
}

/// Deterministic key for a span row.
fn span_id_of(
    file: &str,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    byte_start: u32,
    byte_end: u32,
) -> String {
    deterministic_id(
        "sp",
        &[
            file,
            &start_line.to_string(),
            &start_col.to_string(),
            &end_line.to_string(),
            &end_col.to_string(),
            &byte_start.to_string(),
            &byte_end.to_string(),
        ],
    )
}

/// Deterministic key for a decision: `(candidate, question)` replaces Gel's
/// exclusive constraint of the same shape.
fn decision_key(candidate_id: &str, question_id: &str) -> String {
    deterministic_id("decq", &[candidate_id, question_id])
}

/// Deterministic key for a node-membership relation row.
fn membership_id(build_id: &str, entity_id: &str) -> String {
    deterministic_id("nm", &[build_id, entity_id])
}

/// Deterministic key for an edge-membership relation row.
fn edge_membership_id(build_id: &str, rel_id: &str) -> String {
    deterministic_id("em", &[build_id, rel_id])
}

/// Deterministic key for a claim-evidence link row.
fn link_id(prefix: &str, claim_id: &str, evidence_id: &str) -> String {
    deterministic_id(prefix, &[claim_id, evidence_id])
}

/// Case folding for the `name-fold` search columns (mirrors the Gel `ilike`
/// semantics the reader preserves; ASCII-tested, Unicode `to_lowercase`).
fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// TypeDB-backed [`Store`]: staging validation in memory, durable rows in
/// TypeDB, publication through the active-build pointer.
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
            .map_err(driver_error)?;
        tx.commit().await.map_err(driver_error)?;
        Ok(())
    }

    /// Run one write pipeline and commit. Duplicate `@key` inserts surface
    /// the caller's choice: map them to already-present or propagate.
    async fn write_one(&self, query: &str) -> Result<(), typedb_driver::Error> {
        let driver = self.driver.as_ref().expect("connected before flush");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await?;
        let answer = tx.query(query).await?;
        drain(answer).await?;
        tx.commit().await?;
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
                Err(typedb_driver::Error::Connection(_)) if attempt < FLUSH_RETRIES => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
                Err(e) => return Err(driver_error(e)),
            }
        }
    }

    /// Collect a read query into owned column values. Missing optional
    /// columns (bound through `try {}`) surface as absent keys.
    async fn read_rows(
        &self,
        query: &str,
        columns: &[&str],
    ) -> Result<Vec<BTreeMap<String, Value>>, GelError> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| GelError::Client("TypeDbStore disconnected".into()))?;
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Read,
                TransactionOptions::new().transaction_timeout(READ_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        let answer = tx.query(query).await.map_err(driver_error)?;
        let rows: Vec<ConceptRow> = match answer {
            QueryAnswer::ConceptRowStream(_, stream) => {
                stream.try_collect().await.map_err(driver_error)?
            }
            other => {
                return Err(GelError::Query(format!("expected rows, got {other:?}")));
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut map = BTreeMap::new();
            for col in columns {
                match row.get(col).map_err(driver_error)? {
                    None => {}
                    Some(Concept::Attribute(attr)) => {
                        map.insert((*col).to_owned(), attr.value.clone());
                    }
                    Some(other) => {
                        return Err(GelError::Query(format!(
                            "column {col} is not an attribute: {other:?}"
                        )));
                    }
                }
            }
            out.push(map);
        }
        Ok(out)
    }

    /// Required string column.
    fn col_string(
        row: &BTreeMap<String, Value>,
        col: &str,
    ) -> Result<String, GelError> {
        row.get(col)
            .and_then(Value::get_string)
            .map(str::to_owned)
            .ok_or_else(|| GelError::Query(format!("missing string column {col}")))
    }

    /// Insert the repository row (idempotent).
    async fn flush_repository(&self, repo: &str) -> Result<(), GelError> {
        let q = format!(
            "insert $x isa repository, has repo-name {};",
            str_lit(repo)
        );
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
        let bytes = i64::try_from(bytes).map_err(|_| GelError::Invariant("file size overflows i64".into()))?;
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
    async fn flush_span(
        &self,
        file: &str,
        start_line: u32,
        start_col: u32,
        end_line: u32,
        end_col: u32,
        byte_start: u32,
        byte_end: u32,
    ) -> Result<(), GelError> {
        let conv = |v: u32| int_lit(i64::from(v));
        let q = format!(
            "insert $x isa source-span, has span-id {}, has file {}, has start-line {}, has start-col {}, has end-line {}, has end-col {}, has byte-start {}, has byte-end {};",
            str_lit(&span_id_of(file, start_line, start_col, end_line, end_col, byte_start, byte_end)),
            str_lit(file),
            conv(start_line),
            conv(start_col),
            conv(end_line),
            conv(end_col),
            conv(byte_start),
            conv(byte_end)
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one entity row with its span (idempotent; first write wins,
    /// matching the Gel upsert that keeps the existing row on conflict).
    async fn flush_entity(&self, e: &Entity) -> Result<(), GelError> {
        let s = &e.span;
        self.flush_span(
            &s.file,
            s.start_line,
            s.start_col,
            s.end_line,
            s.end_col,
            s.byte_start,
            s.byte_end,
        )
        .await?;
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
            str_lit(&span_id_of(&s.file, s.start_line, s.start_col, s.end_line, s.end_col, s.byte_start, s.byte_end))
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
    async fn flush_node_membership(
        &self,
        build_id: &str,
        entity_id: &str,
    ) -> Result<(), GelError> {
        let q = format!(
            "match $b isa graph-build, has build-id {}; $e isa code-entity, has entity-id {}; insert (build: $b, member: $e) isa node-membership, has membership-id {};",
            str_lit(build_id),
            str_lit(entity_id),
            str_lit(&membership_id(build_id, entity_id))
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one edge-membership relation (idempotent by deterministic key).
    async fn flush_edge_membership(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<(), GelError> {
        let q = format!(
            "match $b isa graph-build, has build-id {}; $rel isa relationship, has rel-id {}; insert (build: $b, edge: $rel) isa edge-membership, has edge-membership-id {};",
            str_lit(build_id),
            str_lit(rel_id),
            str_lit(&edge_membership_id(build_id, rel_id))
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one extraction-run row (idempotent).
    async fn flush_run(
        &self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
    ) -> Result<(), GelError> {
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
            self.write_one(&q).await.map_err(driver_error)?;
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
            owns.push_str(&format!(", has confidence {}", double_lit(c).map_err(|e| GelError::Query(e.to_string()))?));
        }
        if let Some(p) = d.probability {
            owns.push_str(&format!(", has probability {}", double_lit(p).map_err(|e| GelError::Query(e.to_string()))?));
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
        let rows = self
            .read_rows(&q, &["id", "c", "q", "o", "e", "mr", "mrr", "k", "cf", "p"])
            .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let outcome_str = Self::col_string(&row, "o")?;
        let outcome: DecisionOutcome = serde_json::from_str(&outcome_str)
            .map_err(|e| GelError::Query(format!("bad outcome json: {e}")))?;
        let class_str = Self::col_string(&row, "e")?;
        let evidence_class: EvidenceClass = serde_json::from_str(&format!("\"{class_str}\""))
            .map_err(|e| GelError::Query(format!("bad evidence class: {e}")))?;
        let cache_key = Self::col_string(&row, "k")?;
        let d = Decision {
            id: Self::col_string(&row, "id")?,
            candidate_id: Self::col_string(&row, "c")?,
            question_id: Self::col_string(&row, "q")?,
            outcome,
            evidence_class,
            model_requested: Self::col_string(&row, "mr")?,
            model_returned: Self::col_string(&row, "mrr")?,
            confidence: row.get("cf").and_then(Value::get_double),
            probability: row.get("p").and_then(Value::get_double),
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
            owns.push_str(&format!(
                ", has span-id {}",
                str_lit(&span_id_of(&s.file, s.start_line, s.start_col, s.end_line, s.end_col, s.byte_start, s.byte_end))
            ));
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
            owns.push_str(&format!(", has snapshot-id {}", str_lit(snapshot_id)));
        }
        if let Some(pred) = &build.predecessor {
            owns.push_str(&format!(", has predecessor {}", str_lit(pred)));
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
    async fn live_pointer(
        &self,
        repo: &str,
    ) -> Result<Option<(String, i64, String)>, GelError> {
        let q = format!(
            "match $p isa active-pointer, has repo-name {}, has build-id $b; $g isa graph-build, has build-id $b, has generation $gen, has status $st; select $b, $gen, $st;",
            str_lit(repo)
        );
        let rows = self.read_rows(&q, &["b", "gen", "st"]).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let gen = row
            .get("gen")
            .and_then(Value::get_integer)
            .ok_or_else(|| GelError::Query("pointer build lacks generation".into()))?;
        Ok(Some((Self::col_string(&row, "b")?, gen, Self::col_string(&row, "st")?)))
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
                return Err(GelError::Query(format!("guard expected rows, got {other:?}")));
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
                .map_err(driver_error)?;
        } else {
            let q = format!(
                "match $p isa active-pointer, has repo-name {}; update $p has build-id {}, has updated {};",
                str_lit(repo),
                str_lit(build_id),
                int_lit(now_millis())
            );
            drain(tx.query(&q).await.map_err(driver_error)?)
                .await
                .map_err(driver_error)?;
        }
        let q = format!(
            "match $b isa graph-build, has build-id {}; update $b has status \"active\";",
            str_lit(build_id)
        );
        drain(tx.query(&q).await.map_err(driver_error)?)
            .await
            .map_err(driver_error)?;
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

/// Drain a write answer: rows and documents are collected and dropped so
/// the commit sees a fully-consumed pipeline.
async fn drain(answer: QueryAnswer) -> Result<(), typedb_driver::Error> {
    match answer {
        QueryAnswer::Ok(_) => Ok(()),
        QueryAnswer::ConceptRowStream(_, stream) => {
            let rows: Vec<ConceptRow> = stream.try_collect().await?;
            let _ = rows.len();
            Ok(())
        }
        QueryAnswer::ConceptDocumentStream(_, stream) => {
            let docs: Vec<_> = stream.try_collect().await?;
            let _ = docs.len();
            Ok(())
        }
    }
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
            .ensure_run(run_id, repo, snapshot_id, set_id, catalog_digest, rubric_version)
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
