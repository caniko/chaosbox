//! Complete version checks, including idempotent retries and activation.

use std::collections::{BTreeMap, BTreeSet};
use chaosbox_core::{Claim, GraphBuild};
use chaosbox_store::StoreError;
use typedb_driver::{Transaction, TransactionOptions, TransactionType};
use crate::{
    common::{col_string, col_string_opt, driver_error, read_rows, read_rows_in, READ_TIMEOUT},
    encode::{int_lit, str_lit},
};
use super::TypeDbStore;

impl TypeDbStore {
    pub(super) async fn check_build_digest(
        &self,
        build: &GraphBuild,
        digest: &str,
    ) -> Result<(), StoreError> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("disconnected".into()))?;
        let query = format!(
            "match $b isa graph-build, has build-id {}, has repo-name {}; try {{ $b has publication-digest $digest; }}; select $digest; limit 2;",
            str_lit(&build.id),
            str_lit(&build.repo)
        );
        let rows = read_rows(driver, &self.config.database, &query, &["digest"]).await?;
        if rows.len() != 1 || col_string_opt(&rows[0], "digest").as_deref() != Some(digest) {
            return Err(StoreError::Invariant(
                "build identity reused with a different or unsealed version".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn check_published_version(
        &self,
        build: &GraphBuild,
        digest: &str,
    ) -> Result<(), StoreError> {
        self.check_build_digest(build, digest).await?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("disconnected".into()))?;
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Read,
                TransactionOptions::new().transaction_timeout(READ_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        self.check_memberships(&tx, build, digest).await
    }

    pub(super) async fn check_memberships(
        &self,
        tx: &Transaction,
        build: &GraphBuild,
        digest: &str,
    ) -> Result<(), StoreError> {
        let guard = format!(
            "$b isa graph-build, has build-id {}, has publication-digest {};",
            str_lit(&build.id),
            str_lit(digest)
        );
        let cap = |n: usize| int_lit(i64::try_from(n).unwrap_or(i64::MAX).saturating_add(1));
        let query = format!(
            "match {guard} (build: $b, member: $e) isa node-membership; $e isa code-entity, has entity-id $id; select $id; limit {};",
            cap(build.nodes.len())
        );
        let rows = read_rows_in(tx, &query, &["id"]).await?;
        let ids = rows
            .iter()
            .map(|r| col_string(r, "id"))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if rows.len() != build.nodes.len() || ids != build.nodes.keys().cloned().collect() {
            return Err(StoreError::Invariant(
                "build node membership differs from publication version".into(),
            ));
        }
        let query = format!(
            "match {guard} $m isa edge-membership (build: $b, edge: $e); $e isa relationship, has rel-id $id; try {{ $m has sealed-evidence-json $sealed; }}; select $id, $sealed; limit {};",
            cap(build.edges.len())
        );
        let rows = read_rows_in(tx, &query, &["id", "sealed"]).await?;
        let actual = rows
            .iter()
            .map(|r| Ok((col_string(r, "id")?, col_string(r, "sealed")?)))
            .collect::<Result<BTreeMap<_, _>, StoreError>>()?;
        let expected = self
            .staging
            .publication_evidence(build)?
            .into_iter()
            .map(|(id, rows)| {
                let json = serde_json::to_string(&rows)
                    .map_err(|e| StoreError::Invariant(e.to_string()))?;
                Ok((id, json))
            })
            .collect::<Result<BTreeMap<_, _>, StoreError>>()?;
        if rows.len() != build.edges.len() || actual != expected {
            return Err(StoreError::Invariant(
                "build edge/evidence membership differs from publication version".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn check_claim_payload(&self, claim: &Claim) -> Result<(), StoreError> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("disconnected".into()))?;
        let query = format!(
            "match $c isa claim, has claim-id {}; try {{ $c has claim-json $json; }}; select $json; limit 2;",
            str_lit(&claim.id)
        );
        let rows = read_rows(driver, &self.config.database, &query, &["json"]).await?;
        let json =
            serde_json::to_string(claim).map_err(|e| StoreError::Invariant(e.to_string()))?;
        if !rows.is_empty()
            && (rows.len() != 1
                || col_string_opt(&rows[0], "json").as_deref() != Some(json.as_str()))
        {
            return Err(StoreError::Invariant(
                "claim identity reused with different or unsealed provenance".into(),
            ));
        }
        Ok(())
    }
}
