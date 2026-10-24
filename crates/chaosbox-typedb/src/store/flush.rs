//! Staged-row flush into `TypeDB`: repositories, snapshots, file versions,
//! spans, entities, relationships, memberships, runs, sets, candidates.
//!
//! Content-keyed rows are built here as [`KeyedRow`]s and written in chunks
//! by [`TypeDbStore::flush_keyed_rows`]; the handful of singleton rows per
//! publication (repository, snapshot, run, set) stay on the per-row path.

use chaosbox_core::{Candidate, Entity, Relation, SourceSpan, relation_type_name};
use chaosbox_store::StoreError;
use crate::common::{edge_membership_id, file_version_id, fold, membership_id, now_millis, span_id_of};
use crate::encode::{int_lit, str_lit};

use super::{KeyedRow, TypeDbStore};

fn span_key(span: &SourceSpan) -> String {
    span_id_of(
        &span.file,
        span.start_line,
        span.start_col,
        span.end_line,
        span.end_col,
        span.byte_start,
        span.byte_end,
    )
}

/// One span row (keyed by position; shared across snapshots of an
/// unchanged file, which is why the probe matters more here than anywhere).
pub(super) fn span_row(span: &SourceSpan) -> KeyedRow {
    let conv = |v: u32| int_lit(i64::from(v));
    let key = span_key(span);
    let insert = format!(
        "insert $x isa source-span, has span-id {}, has file {}, has start-line {}, has start-col {}, has end-line {}, has end-col {}, has byte-start {}, has byte-end {};",
        str_lit(&key),
        str_lit(&span.file),
        conv(span.start_line),
        conv(span.start_col),
        conv(span.end_line),
        conv(span.end_col),
        conv(span.byte_start),
        conv(span.byte_end)
    );
    KeyedRow::new("source-span", "span-id", &key, insert)
}

/// One entity row. Its span row must be flushed first (see
/// `flush_build_rows`).
pub(super) fn entity_row(e: &Entity) -> Result<KeyedRow, StoreError> {
    let kind = serde_json::to_value(&e.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{:?}", e.kind));
    let compiler = e
        .compiler
        .as_ref()
        .map(|value| {
            serde_json::to_string(value)
                .map(|json| format!(", has compiler-json {}", str_lit(&json)))
        })
        .transpose()
        .map_err(|e| StoreError::Invariant(e.to_string()))?
        .unwrap_or_default();
    let insert = format!(
        "insert $x isa code-entity, has entity-id {}, has kind {}, has repo-name {}, has snapshot-id {}, has file {}, has name {}, has name-fold {}, has qualified-name {}, has qualified-name-fold {}, has span-id {}{compiler};",
        str_lit(&e.id),
        str_lit(&kind),
        str_lit(&e.repo),
        str_lit(&e.snapshot),
        str_lit(&e.file),
        str_lit(&e.name),
        str_lit(&fold(&e.name)),
        str_lit(&e.qualified_name),
        str_lit(&fold(&e.qualified_name)),
        str_lit(&span_key(&e.span))
    );
    Ok(KeyedRow::new("code-entity", "entity-id", &e.id, insert))
}

/// One relationship with typed endpoint roles; both entities must already
/// be flushed.
pub(super) fn relationship_row(r: &Relation) -> KeyedRow {
    let rel_type = relation_type_name(&r.rel_type);
    let scope = serde_json::to_value(&r.scope)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{:?}", r.scope));
    let insert = format!(
        "match $f isa code-entity, has entity-id {}; $t isa code-entity, has entity-id {}; insert (from-entity: $f, to-entity: $t) isa relationship, has rel-id {}, has rel-type {}, has scope {};",
        str_lit(&r.from),
        str_lit(&r.to),
        str_lit(&r.id),
        str_lit(&rel_type),
        str_lit(&scope)
    );
    KeyedRow::new("relationship", "rel-id", &r.id, insert)
}

/// One node-membership relation (deterministic key per build and entity).
pub(super) fn node_membership_row(build_id: &str, entity_id: &str, digest: &str) -> KeyedRow {
    let key = membership_id(build_id, entity_id);
    let insert = format!(
        "match $b isa graph-build, has build-id {}, has publication-digest {}, has status \"staging\"; $e isa code-entity, has entity-id {}; insert (build: $b, member: $e) isa node-membership, has membership-id {};",
        str_lit(build_id),
        str_lit(digest),
        str_lit(entity_id),
        str_lit(&key)
    );
    KeyedRow::new("node-membership", "membership-id", &key, insert)
}

/// One file-version row (deterministic key per snapshot and path).
pub(super) fn file_version_row(
    snapshot_id: &str,
    path: &str,
    sha: &str,
    bytes: u64,
) -> Result<KeyedRow, StoreError> {
    let bytes = i64::try_from(bytes)
        .map_err(|_| StoreError::Invariant("file size overflows i64".into()))?;
    let key = file_version_id(snapshot_id, path);
    let insert = format!(
        "insert $x isa file-version, has file-version-id {}, has snapshot-id {}, has path {}, has sha256 {}, has bytes {};",
        str_lit(&key),
        str_lit(snapshot_id),
        str_lit(path),
        str_lit(sha),
        int_lit(bytes)
    );
    Ok(KeyedRow::new(
        "file-version",
        "file-version-id",
        &key,
        insert,
    ))
}

/// One candidate row.
pub(super) fn candidate_row(set_id: &str, c: &Candidate) -> KeyedRow {
    let insert = format!(
        "insert $x isa candidate, has candidate-id {}, has set-id {}, has rel-type {}, has from-entity-id {}, has to-entity-id {}, has reason {}, has state-excerpt {};",
        str_lit(&c.id),
        str_lit(set_id),
        str_lit(&relation_type_name(&c.rel_type)),
        str_lit(&c.from_entity),
        str_lit(&c.to_entity),
        str_lit(&c.reason),
        str_lit(&c.state_excerpt)
    );
    KeyedRow::new("candidate", "candidate-id", &c.id, insert)
}

impl TypeDbStore {
    /// Insert the repository row (idempotent).
    pub(super) async fn flush_repository(&self, repo: &str) -> Result<(), StoreError> {
        let q = format!("insert $x isa repository, has repo-name {};", str_lit(repo));
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one snapshot row (idempotent).
    pub(super) async fn flush_snapshot(
        &self,
        repo: &str,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
        let q = format!(
            "insert $x isa source-snapshot, has snapshot-id {}, has repo-name {}, has created {};",
            str_lit(snapshot_id),
            str_lit(repo),
            int_lit(now_millis())
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// One edge-membership relation carrying the sealed evidence for its
    /// edge. The publication swing's `check_memberships` re-reads every
    /// edge's sealed JSON inside the swing transaction, so a conflicting or
    /// unsealed row still fails publication without a per-edge read here.
    pub(super) fn edge_membership_row(
        &self,
        build_id: &str,
        rel_id: &str,
        digest: &str,
    ) -> Result<KeyedRow, StoreError> {
        let sealed = serde_json::to_string(self.staging.sealed_evidence(build_id, rel_id)?)
            .map_err(|e| StoreError::Invariant(e.to_string()))?;
        let key = edge_membership_id(build_id, rel_id);
        let insert = format!(
            "match $b isa graph-build, has build-id {}, has publication-digest {}, has status \"staging\"; $rel isa relationship, has rel-id {}; insert (build: $b, edge: $rel) isa edge-membership, has edge-membership-id {}, has sealed-evidence-json {};",
            str_lit(build_id),
            str_lit(digest),
            str_lit(rel_id),
            str_lit(&key),
            str_lit(&sealed),
        );
        Ok(KeyedRow::new(
            "edge-membership",
            "edge-membership-id",
            &key,
            insert,
        ))
    }

    /// Insert one extraction-run row (idempotent).
    pub(super) async fn flush_run(
        &self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
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
    pub(super) async fn flush_set(
        &self,
        set_id: &str,
        run_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), StoreError> {
        let q = format!(
            "insert $x isa candidate-set, has set-id {}, has run-id {}, has catalog-digest {}, has rubric-version {};",
            str_lit(set_id),
            str_lit(run_id),
            str_lit(catalog_digest),
            str_lit(rubric_version)
        );
        self.insert_ignoring_duplicates(&q).await
    }
}
