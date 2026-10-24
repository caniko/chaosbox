//! Read-only PostgreSQL graph queries and scoped private knowledge.
//!
//! Every read is one statement scoped to a pinned build. Published rows are
//! immutable, so a pinned build reads consistently across statements without
//! holding a transaction. Ordering matches the reference surface: entity lists
//! sort by qualified name, relationship and evidence lists by id ("C"
//! collation, identical to Rust byte order).

use std::sync::Arc;

use chaosbox_store::{
    BuildRow, EndpointRef, EntityRow, EvidenceRow, GraphQueries, NeighborhoodDirection, RelRow,
    StoreError,
};
use tokio_postgres::{error::SqlState, Client, GenericClient, Row};

use crate::common::{connect, entity_from_row, pg_error, vector_literal, PgConfig, ENTITY_COLUMNS};

#[path = "inventory.rs"]
mod inventory;
pub use inventory::{InventoryPage, InventoryQuery, RetainedBuild, INVENTORY_TIMEOUT};

/// Undo `%`-wrapping and `\` escapes of a `like` pattern into a literal
/// substring needle (mirrors the in-memory reader).
fn unescape_like(like: &str) -> String {
    let mut out = String::with_capacity(like.len());
    let mut chars = like.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else if c != '%' {
            out.push(c);
        }
    }
    out
}

fn rel_from_row(row: &Row) -> RelRow {
    RelRow {
        rel_id: row.get(0),
        rel_type: row.get(1),
        from_entity: EndpointRef {
            entity_id: row.get(2),
        },
        to_entity: EndpointRef {
            entity_id: row.get(3),
        },
    }
}

const REL_COLUMNS: &str = "rel_id, rel_type, from_id, to_id";

/// Cosine similarity of `query` to stored texts under `model`.
pub(crate) async fn rank_texts<C: GenericClient + Sync>(
    client: &C,
    model: &str,
    query: &[f32],
    shas: &[String],
) -> Result<Vec<(String, f64)>, StoreError> {
    let query = vector_literal(query)?;
    Ok(client
        .query(
            "SELECT text_sha256, 1 - (vec <=> $2::text::halfvec) FROM embeddings WHERE model = $1 AND text_sha256 = ANY($3)",
            &[&model, &query, &shas],
        )
        .await
        .map_err(pg_error)?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect())
}

/// Read-only query handle. Clones share one session; statements pipeline.
#[derive(Clone)]
pub struct PgReader {
    config: PgConfig,
    client: Option<Arc<Client>>,
}

impl PgReader {
    /// A disconnected reader; connects on [`PgReader::connect`].
    #[must_use]
    pub fn new(config: PgConfig) -> Self {
        Self {
            config,
            client: None,
        }
    }

    /// Whether an operator-selected configuration names the same connection.
    /// This grants no build pin or scoped-read authority.
    #[must_use]
    pub fn matches_config(&self, config: &PgConfig) -> bool {
        self.config == *config
    }

    /// True once the session has failed; a cached handle must be replaced.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.client.as_ref().is_none_or(|c| c.is_closed())
    }

    /// Connect. The read path never creates databases or applies schemas: a
    /// missing database is a "not found" client error the caller maps to the
    /// pending contract.
    pub async fn connect(&mut self) -> Result<(), StoreError> {
        if self.is_closed() {
            let client = connect(&self.config).await?;
            client
                .batch_execute("SET default_transaction_read_only = on")
                .await
                .map_err(pg_error)?;
            self.client = Some(Arc::new(client));
        }
        Ok(())
    }

    fn client(&self) -> Result<&Client, StoreError> {
        self.client
            .as_deref()
            .ok_or_else(|| StoreError::Connection("PgReader disconnected".into()))
    }

    /// The private scope's current knowledge generation.
    pub async fn knowledge(&self, scope: &str) -> Result<Option<(String, String)>, StoreError> {
        crate::knowledge::current(self.client()?, scope).await
    }

    /// One exact scoped generation without advancing its pointer.
    pub async fn knowledge_at(&self, scope: &str, id: &str) -> Result<Option<String>, StoreError> {
        crate::knowledge::at(self.client()?, scope, id).await
    }

    /// Cosine similarity of `query` to each stored text under `model`.
    pub async fn rank_texts(
        &self,
        model: &str,
        query: &[f32],
        shas: &[String],
    ) -> Result<Vec<(String, f64)>, StoreError> {
        rank_texts(self.client()?, model, query, shas).await
    }

    /// Schema probe: true when this binary's schema version is applied, false
    /// before migration. Connection failures propagate.
    pub async fn probe(&self) -> Result<bool, StoreError> {
        match self
            .client()?
            .query_one("SELECT max(version) FROM schema_version", &[])
            .await
        {
            Ok(row) => Ok(row
                .get::<_, Option<i32>>(0)
                .is_some_and(|v| u32::try_from(v).is_ok_and(|v| v >= crate::SCHEMA_VERSION))),
            Err(e) if e.code() == Some(&SqlState::UNDEFINED_TABLE) => Ok(false),
            Err(e) => Err(pg_error(e)),
        }
    }

    fn header(row: Option<Row>) -> Result<Option<BuildRow>, StoreError> {
        let Some(row) = row else {
            return Ok(None);
        };
        let mut snapshots: Vec<String> = row.get(2);
        snapshots.sort();
        let coverage: Option<String> = row.get(3);
        Ok(Some(BuildRow {
            build_id: row.get(0),
            generation: row.get(1),
            status: "active".into(),
            evidence_sealed: true,
            snapshots,
            coverage: coverage
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .map_err(|e| StoreError::Query(format!("invalid build coverage: {e}")))?,
        }))
    }

    async fn rels(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<RelRow>, StoreError> {
        Ok(self
            .client()?
            .query(sql, params)
            .await
            .map_err(pg_error)?
            .iter()
            .map(rel_from_row)
            .collect())
    }

    async fn entities(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<EntityRow>, StoreError> {
        self.client()?
            .query(sql, params)
            .await
            .map_err(pg_error)?
            .iter()
            .map(entity_from_row)
            .collect()
    }
}

#[async_trait::async_trait]
impl GraphQueries for PgReader {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError> {
        let row = self
            .client()?
            .query_opt(
                "SELECT b.build_id, b.generation, b.snapshot_ids, b.coverage FROM active_pointer p JOIN graph_builds b ON b.build_id = p.build_id AND b.repo = p.repo WHERE p.repo = $1",
                &[&repo],
            )
            .await
            .map_err(pg_error)?;
        Self::header(row)
    }

    async fn published_build(
        &self,
        repo: &str,
        build_id: &str,
    ) -> Result<Option<BuildRow>, StoreError> {
        let row = self
            .client()?
            .query_opt(
                "SELECT build_id, generation, snapshot_ids, coverage FROM graph_builds WHERE build_id = $1 AND repo = $2",
                &[&build_id, &repo],
            )
            .await
            .map_err(pg_error)?;
        Self::header(row)
    }

    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        let needle = crate::common::fold(&unescape_like(like));
        self.entities(
            &format!(
                "SELECT {ENTITY_COLUMNS} FROM node_members m JOIN entities e ON e.entity_id = m.entity_id WHERE m.build_id = $1 AND (strpos(e.name_fold, $2) > 0 OR strpos(e.qn_fold, $2) > 0) ORDER BY e.name_fold = $2 DESC, starts_with(e.name_fold, $2) DESC, e.qualified_name, e.entity_id LIMIT $3"
            ),
            &[&build_id, &needle, &limit.max(0)],
        )
        .await
    }

    async fn entity_by_id(
        &self,
        build_id: &str,
        id: &str,
    ) -> Result<Option<EntityRow>, StoreError> {
        Ok(self
            .entities(
                &format!(
                    "SELECT {ENTITY_COLUMNS} FROM node_members m JOIN entities e ON e.entity_id = m.entity_id WHERE m.build_id = $1 AND m.entity_id = $2"
                ),
                &[&build_id, &id],
            )
            .await?
            .pop())
    }

    async fn neighbor_counts(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<(u64, u64), StoreError> {
        let row = self
            .client()?
            .query_one(
                "SELECT count(*) FILTER (WHERE from_id = $2), count(*) FILTER (WHERE to_id = $2) FROM relationships WHERE build_id = $1 AND (from_id = $2 OR to_id = $2) AND rel_type = ANY($3)",
                &[&build_id, &id, &rel_types],
            )
            .await
            .map_err(pg_error)?;
        let count = |i| {
            u64::try_from(row.get::<_, i64>(i))
                .map_err(|_| StoreError::Query("negative relationship count".into()))
        };
        Ok((count(0)?, count(1)?))
    }

    async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        self.rels(
            &format!("SELECT {REL_COLUMNS} FROM relationships WHERE build_id = $1 AND from_id = $2 AND rel_type = ANY($3) ORDER BY rel_id"),
            &[&build_id, &id, &rel_types],
        )
        .await
    }

    async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        self.rels(
            &format!("SELECT {REL_COLUMNS} FROM relationships WHERE build_id = $1 AND to_id = $2 AND rel_type = ANY($3) ORDER BY rel_id"),
            &[&build_id, &id, &rel_types],
        )
        .await
    }

    async fn neighbor_page(
        &self,
        build_id: &str,
        id: &str,
        direction: NeighborhoodDirection,
        rel_types: Vec<String>,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        if limit <= 0 || rel_types.is_empty() {
            return Ok(Vec::new());
        }
        let anchor = match direction {
            NeighborhoodDirection::Outgoing => "from_id",
            NeighborhoodDirection::Incoming => "to_id",
        };
        self.rels(
            &format!("SELECT {REL_COLUMNS} FROM relationships WHERE build_id = $1 AND {anchor} = $2 AND rel_type = ANY($3) AND ($4::text IS NULL OR rel_id > $4) ORDER BY rel_id LIMIT $5"),
            &[&build_id, &id, &rel_types, &after, &limit],
        )
        .await
    }

    async fn build_entities(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        self.entities(
            &format!(
                "SELECT {ENTITY_COLUMNS} FROM node_members m JOIN entities e ON e.entity_id = m.entity_id WHERE m.build_id = $1 ORDER BY e.qualified_name, e.entity_id LIMIT $2"
            ),
            &[&build_id, &limit.max(0)],
        )
        .await
    }

    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        self.rels(
            &format!("SELECT {REL_COLUMNS} FROM relationships WHERE build_id = $1 ORDER BY rel_id LIMIT $2"),
            &[&build_id, &limit.max(0)],
        )
        .await
    }

    async fn adjacent_relationships(
        &self,
        build_id: &str,
        id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        // Ordered by (neighbor, rel id); a self-edge appears once.
        self.rels(
            &format!("SELECT {REL_COLUMNS} FROM relationships WHERE build_id = $1 AND (from_id = $2 OR to_id = $2) ORDER BY CASE WHEN from_id = $2 THEN to_id ELSE from_id END, rel_id LIMIT $3"),
            &[&build_id, &id, &limit.max(0)],
        )
        .await
    }

    async fn nearest_entities(
        &self,
        build_id: &str,
        model: &str,
        query: &[f32],
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        // ponytail: exact scan over the build's embedded entities. Add an HNSW
        // index with `hnsw.iterative_scan` when p50 latency is measured slow;
        // a filtered HNSW scan without iterative scan silently loses recall.
        let query = vector_literal(query)?;
        self.entities(
            &format!(
                "SELECT {ENTITY_COLUMNS} FROM node_members m JOIN entities e ON e.entity_id = m.entity_id JOIN embeddings v ON v.text_sha256 = e.text_sha256 AND v.model = $2 WHERE m.build_id = $1 ORDER BY v.vec <=> $3::text::halfvec, e.entity_id LIMIT $4"
            ),
            &[&build_id, &model, &query, &limit.max(0)],
        )
        .await
    }

    async fn navigation_summary(
        &self,
        build_id: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let row = self
            .client()?
            .query_opt(
                "SELECT navigation FROM graph_builds WHERE build_id = $1",
                &[&build_id],
            )
            .await
            .map_err(pg_error)?;
        row.map(|r| {
            let json: String = r.get(0);
            if json.len() > chaosbox_core::navigation::SUMMARY_BYTES_MAX {
                return Err(StoreError::QueryBudget);
            }
            serde_json::from_str(&json)
                .map_err(|_| StoreError::Query("invalid publication analytics".into()))
        })
        .transpose()
    }

    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        self.evidence_for_limited(build_id, rel_id, i64::MAX).await
    }

    async fn evidence_for_limited(
        &self,
        build_id: &str,
        rel_id: &str,
        limit: i64,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        // Publication stores the complete evidence closure on the exact build
        // membership; mutable claims are never re-joined.
        let Some(row) = self
            .client()?
            .query_opt(
                "SELECT sealed_evidence FROM relationships WHERE build_id = $1 AND rel_id = $2",
                &[&build_id, &rel_id],
            )
            .await
            .map_err(pg_error)?
        else {
            return Ok(Vec::new());
        };
        let json: String = row.get(0);
        if json.len() > chaosbox_store::SEALED_EVIDENCE_BYTES_MAX {
            return Err(StoreError::QueryBudget);
        }
        let mut evidence: Vec<EvidenceRow> =
            serde_json::from_str(&json).map_err(|_| StoreError::EvidenceClosureUnavailable)?;
        // Duplicate or unsorted ids mean a corrupted packet, never a silent drop.
        if evidence
            .windows(2)
            .any(|w| w[0].evidence_id >= w[1].evidence_id)
        {
            return Err(StoreError::EvidenceClosureUnavailable);
        }
        evidence.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(evidence)
    }
}
