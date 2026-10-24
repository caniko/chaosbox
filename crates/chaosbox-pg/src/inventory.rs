//! Operator-only retained-build metadata. Counts are logical memberships
//! recorded at publication, not storage bytes.

use std::time::Duration;

use chaosbox_store::StoreError;
use serde::Serialize;

use super::PgReader;
use crate::common::pg_error;

/// Whole CLI request deadline.
pub const INVENTORY_TIMEOUT: Duration = Duration::from_secs(10);

/// Validated metadata request. Construction precedes backend acquisition.
pub struct InventoryQuery {
    repo: String,
    limit: usize,
    after: Option<String>,
}

impl InventoryQuery {
    /// Select one repository and an exclusive build-ID boundary, with 1..=50 rows.
    pub fn new(repo: String, limit: usize, after: Option<String>) -> Result<Self, StoreError> {
        if !(1..=50).contains(&limit)
            || std::iter::once(repo.as_str())
                .chain(after.as_deref())
                .any(|label| {
                    label.trim().is_empty()
                        || label.len() > 256
                        || label.chars().any(char::is_control)
                })
        {
            return Err(StoreError::Query("invalid build inventory: nonblank control-free labels up to 256 bytes and limit 1..=50 required".into()));
        }
        Ok(Self { repo, limit, after })
    }
}

/// One published build header.
#[derive(Debug, Serialize)]
pub struct RetainedBuild {
    /// Opaque stored identity.
    pub build_id: String,
    /// Stored generation; it is not the number of retained builds.
    pub generation: i64,
    /// Always `active`: only published builds are stored.
    pub status: String,
    /// Whether this ID equals the repository pointer in this page's snapshot.
    pub is_current: bool,
    /// Stored predecessor, when present.
    pub predecessor: Option<String>,
    /// Number of node memberships, not unique physical entities across builds.
    pub node_memberships: i64,
    /// Number of edge memberships, not unique physical relationships across builds.
    pub edge_memberships: i64,
    /// Edge memberships carrying a sealed payload (every published edge).
    pub sealed_memberships: i64,
}

/// A bounded metadata page observed in one statement snapshot.
#[derive(Debug, Serialize)]
pub struct InventoryPage {
    /// Requested repository.
    pub repo: String,
    /// Pointer value.
    pub current_build: Option<String>,
    /// Stored headers in ascending build-ID order.
    pub builds: Vec<RetainedBuild>,
    /// True when this snapshot has no further headers past the boundary.
    pub complete: bool,
    /// Exclusive boundary for the next request; never an authorization credential.
    pub next_after: Option<String>,
    /// Unmeasured. Logical membership counts do not attribute engine storage.
    pub physical_bytes: Option<u64>,
}

impl PgReader {
    /// Read only bounded headers and recorded counts, never payloads.
    pub async fn inventory(&self, query: &InventoryQuery) -> Result<InventoryPage, StoreError> {
        let limit = i64::try_from(query.limit + 1).expect("bounded limit");
        let rows = self
            .client()?
            .query(
                "SELECT p.build_id, b.build_id, b.generation, b.predecessor, b.node_count, b.edge_count FROM (SELECT $1::text AS repo) r LEFT JOIN active_pointer p ON p.repo = r.repo LEFT JOIN LATERAL (SELECT * FROM graph_builds g WHERE g.repo = r.repo AND ($2::text IS NULL OR g.build_id > $2) ORDER BY g.build_id LIMIT $3) b ON true ORDER BY b.build_id",
                &[&query.repo, &query.after, &limit],
            )
            .await
            .map_err(pg_error)?;
        let current_build: Option<String> = rows.first().and_then(|r| r.get(0));
        let mut builds: Vec<RetainedBuild> = rows
            .iter()
            .filter_map(|r| {
                let build_id: Option<String> = r.get(1);
                build_id.map(|build_id| RetainedBuild {
                    is_current: current_build.as_ref() == Some(&build_id),
                    build_id,
                    generation: r.get(2),
                    status: "active".into(),
                    predecessor: r.get(3),
                    node_memberships: r.get(4),
                    edge_memberships: r.get(5),
                    sealed_memberships: r.get(5),
                })
            })
            .collect();
        let complete = builds.len() <= query.limit;
        builds.truncate(query.limit);
        let next_after = if complete {
            None
        } else {
            builds.last().map(|row| row.build_id.clone())
        };
        Ok(InventoryPage {
            repo: query.repo.clone(),
            current_build,
            builds,
            complete,
            next_after,
            physical_bytes: None,
        })
    }
}
