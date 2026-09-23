//! Staged-row flush into `TypeDB`: repositories, snapshots, file versions,
//! spans, entities, relationships, memberships, runs, sets, candidates.

use chaosbox_core::{Candidate, Entity, Relation, relation_type_name};
use chaosbox_store::StoreError;
use crate::common::{edge_membership_id, file_version_id, fold, membership_id, now_millis, span_id_of};
use crate::encode::{int_lit, str_lit};

use super::TypeDbStore;

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

    /// Insert one file-version row (idempotent by deterministic key).
    pub(super) async fn flush_file_version(
        &self,
        snapshot_id: &str,
        path: &str,
        sha: &str,
        bytes: u64,
    ) -> Result<(), StoreError> {
        let bytes = i64::try_from(bytes)
            .map_err(|_| StoreError::Invariant("file size overflows i64".into()))?;
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
    pub(super) async fn flush_span(
        &self,
        span: &chaosbox_core::SourceSpan,
    ) -> Result<(), StoreError> {
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
    /// first write wins: the existing row is kept on conflict).
    pub(super) async fn flush_entity(&self, e: &Entity) -> Result<(), StoreError> {
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
    pub(super) async fn flush_relationship(&self, r: &Relation) -> Result<(), StoreError> {
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
    pub(super) async fn flush_node_membership(
        &self,
        build_id: &str,
        entity_id: &str,
    ) -> Result<(), StoreError> {
        let q = format!(
            "match $b isa graph-build, has build-id {}; $e isa code-entity, has entity-id {}; insert (build: $b, member: $e) isa node-membership, has membership-id {};",
            str_lit(build_id),
            str_lit(entity_id),
            str_lit(&membership_id(build_id, entity_id))
        );
        self.insert_ignoring_duplicates(&q).await
    }

    /// Insert one edge-membership relation (idempotent by deterministic key).
    pub(super) async fn flush_edge_membership(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<(), StoreError> {
        let q = format!(
            "match $b isa graph-build, has build-id {}; $rel isa relationship, has rel-id {}; insert (build: $b, edge: $rel) isa edge-membership, has edge-membership-id {};",
            str_lit(build_id),
            str_lit(rel_id),
            str_lit(&edge_membership_id(build_id, rel_id))
        );
        self.insert_ignoring_duplicates(&q).await
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

    /// Insert one candidate row (idempotent).
    pub(super) async fn flush_candidate(
        &self,
        set_id: &str,
        c: &Candidate,
    ) -> Result<(), StoreError> {
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
}
