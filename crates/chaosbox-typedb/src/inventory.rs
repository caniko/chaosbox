//! Operator-only retained-build metadata. Counts are logical memberships, not storage bytes.

use std::time::Duration;
use serde::Serialize;
use typedb_driver::{TransactionOptions, TransactionType};
use chaosbox_store::StoreError;
use crate::common::{col_int, col_string, col_string_opt, driver_error, read_rows_in};
use crate::encode::{int_lit, str_lit};
use super::TypeDbReader;

/// Whole CLI request deadline, also used for the read transaction's lifetime.
pub const INVENTORY_TIMEOUT: Duration = Duration::from_secs(10);

/// Validated metadata request. Construction precedes credential/backend acquisition.
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

/// One stored header, including unpublished headers. This is not publication admission.
#[derive(Debug, Serialize)]
pub struct RetainedBuild {
    /// Opaque stored identity.
    pub build_id: String,
    /// Stored generation; it is not the number of retained builds.
    pub generation: i64,
    /// Stored status; historical published builds also retain `active` status.
    pub status: String,
    /// Whether this ID equals the repository pointer in this page's read transaction.
    pub is_current: bool,
    /// Stored predecessor, when present.
    pub predecessor: Option<String>,
    /// Number of node memberships, not unique physical entities across builds.
    pub node_memberships: i64,
    /// Number of edge memberships, not unique physical relationships across builds.
    pub edge_memberships: i64,
    /// Edge memberships carrying a sealed payload, without fetching that payload.
    pub sealed_memberships: i64,
}

/// A bounded metadata page observed within one read transaction. Subsequent pages
/// take fresh read snapshots; concurrent writers are not frozen by this command.
#[derive(Debug, Serialize)]
pub struct InventoryPage {
    /// Requested repository.
    pub repo: String,
    /// Pointer value, independent of stored build status.
    pub current_build: Option<String>,
    /// Stored headers in ascending build-ID order.
    pub builds: Vec<RetainedBuild>,
    /// True when this read snapshot has no further headers past the boundary.
    pub complete: bool,
    /// Exclusive boundary for the next request; never an authorization credential.
    pub next_after: Option<String>,
    /// Unmeasured. Logical membership counts do not attribute engine storage.
    pub physical_bytes: Option<u64>,
}

impl TypeDbReader {
    /// Read only bounded headers and scalar counts, never payloads or a full export.
    pub async fn inventory(&self, query: &InventoryQuery) -> Result<InventoryPage, StoreError> {
        let tx = self
            .driver()?
            .transaction_with_options(
                &self.config.database,
                TransactionType::Read,
                TransactionOptions::new().transaction_timeout(INVENTORY_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        let pointer = read_rows_in(
            &tx,
            &format!(
            "match $p isa active-pointer, has repo-name {}, has build-id $id; select $id; limit 2;",
            str_lit(&query.repo)),
            &["id"],
        )
        .await?;
        if pointer.len() > 1 {
            return Err(StoreError::Query("multiple repository pointers".into()));
        }
        let current_build = pointer
            .first()
            .map(|row| col_string(row, "id"))
            .transpose()?;
        let boundary = query
            .after
            .as_ref()
            .map_or_else(String::new, |id| format!("$id > {};", str_lit(id)));
        let mut headers = read_rows_in(&tx, &format!(
            "match $b isa graph-build, has repo-name {}, has build-id $id, has generation $gen, has status $status; try {{ $b has predecessor $pred; }}; {boundary} select $id, $gen, $status, $pred; sort $id; limit {};",
            str_lit(&query.repo), int_lit(i64::try_from(query.limit + 1).unwrap_or(51))),
            &["id", "gen", "status", "pred"]).await?;
        let complete = headers.len() <= query.limit;
        headers.truncate(query.limit);
        let mut builds = Vec::with_capacity(headers.len());
        // ponytail: three scalar queries per returned header (at most 50); use
        // grouped backend aggregates if measured round trips justify the change.
        for row in headers {
            let build_id = col_string(&row, "id")?;
            let mut counts = [0; 3];
            for (slot, membership) in counts.iter_mut().zip([
                "$m isa node-membership (build: $b)",
                "$m isa edge-membership (build: $b)",
                "$m isa edge-membership (build: $b), has sealed-evidence-json $sealed",
            ]) {
                let rows = read_rows_in(&tx, &format!(
                    "match $b isa graph-build, has build-id {}; {membership}; reduce $count = count($m);",
                    str_lit(&build_id)), &["count"]).await?;
                if rows.len() != 1 {
                    return Err(StoreError::Query("invalid inventory count result".into()));
                }
                *slot = col_int(&rows[0], "count")?;
                if *slot < 0 {
                    return Err(StoreError::Query("negative inventory count".into()));
                }
            }
            builds.push(RetainedBuild {
                is_current: current_build.as_ref() == Some(&build_id),
                build_id,
                generation: col_int(&row, "gen")?,
                status: col_string(&row, "status")?,
                predecessor: col_string_opt(&row, "pred"),
                node_memberships: counts[0],
                edge_memberships: counts[1],
                sealed_memberships: counts[2],
            });
        }
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
