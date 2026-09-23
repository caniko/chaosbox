//! Flush chain driver: drains the memory staging area in dependency order,
//! then the active-pointer read and the atomic publication swing.

use chaosbox_store::StoreError;
use futures::TryStreamExt;
use typedb_driver::{
    TransactionOptions, TransactionType,
    answer::{ConceptRow, QueryAnswer},
};
use crate::common::{
    WRITE_TIMEOUT, col_int, col_string, drain, driver_error, is_conflict, now_millis, read_rows,
};
use crate::encode::{int_lit, str_lit};

use super::TypeDbStore;

impl TypeDbStore {
    /// Flush staged runs, sets, candidates, decisions, evidence, and claims
    /// in dependency order (all idempotent; safe to retry after a crash).
    pub(super) async fn flush_chain(&self) -> Result<(), StoreError> {
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
    pub(super) async fn live_pointer(
        &self,
        repo: &str,
    ) -> Result<Option<(String, i64, String)>, StoreError> {
        let q = format!(
            "match $p isa active-pointer, has repo-name {}, has build-id $b; $g isa graph-build, has build-id $b, has generation $gen, has status $st; select $b, $gen, $st;",
            str_lit(repo)
        );
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("TypeDbStore disconnected".into()))?;
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
    pub(super) async fn swing(
        &self,
        repo: &str,
        build_id: &str,
        expected: Option<(String, i64)>,
        generation: i64,
    ) -> Result<(), StoreError> {
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
                return Err(StoreError::Query(format!(
                    "guard expected rows, got {other:?}"
                )));
            }
        };
        if guard_rows.is_empty() {
            return Err(StoreError::Invariant(
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
            Err(e) if is_conflict(&e) => Err(StoreError::Invariant(
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
