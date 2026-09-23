//! The [`Store`] trait implementation: staging writes, read-side queries,
//! and atomic publication.

use chaosbox_core::{Candidate, Claim, Decision, Entity, Evidence, GraphBuild, Relation, SnapshotFile};
use chaosbox_store::{Store, StoreError, StoreStats};
use crate::common::decision_key;
use crate::encode::double_lit;

use super::TypeDbStore;

#[async_trait::async_trait]
impl Store for TypeDbStore {
    async fn ensure_snapshot_files(
        &mut self,
        snapshot_id: &str,
        repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), StoreError> {
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
    ) -> Result<(), StoreError> {
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

    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), StoreError> {
        self.staging.put_candidate(set_id, c).await
    }

    async fn put_entity(&mut self, e: Entity) -> Result<(), StoreError> {
        self.staging.put_entity(e).await
    }

    async fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), StoreError> {
        self.staging.put_relation(r, build_id).await
    }

    async fn put_decision(&mut self, d: Decision) -> Result<(), StoreError> {
        // Non-finite floats have no TypeQL form: reject at the boundary
        // instead of failing mid-flush.
        if let Some(c) = d.confidence {
            double_lit(c).map_err(|e| StoreError::Invariant(e.to_string()))?;
        }
        if let Some(p) = d.probability {
            double_lit(p).map_err(|e| StoreError::Invariant(e.to_string()))?;
        }
        self.staging.put_decision(d.clone()).await?;
        // Write-through: the decision is paid inference — persist it in a
        // short transaction as produced, so a dead worker or a budget
        // failure later in the run never loses completed work (resume
        // reuses it through `find_decision`). Staged rows still flush
        // idempotently at publish; readers pin builds, so a decision
        // written before publication stays invisible to them.
        self.ensure_connected().await?;
        self.flush_decision(&d).await
    }

    async fn put_evidence(&mut self, e: Evidence) -> Result<(), StoreError> {
        self.staging.put_evidence(e.clone()).await?;
        // Write-through alongside its decision (deterministic evidence id:
        // re-assembly and the publish-time flush stay idempotent).
        self.ensure_connected().await?;
        self.flush_evidence(&e).await
    }

    async fn put_claim(&mut self, c: Claim) -> Result<(), StoreError> {
        self.staging.put_claim(c).await
    }

    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, StoreError> {
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
    ) -> Result<(), StoreError> {
        // 1. Live-pointer guard before writing anything (idempotent retry
        // returns early, stale predecessors fail here).
        self.ensure_connected().await?;
        let live = self.live_pointer(&build.repo).await?;
        let pred_gen = match (&live, &expected_predecessor) {
            (None, None) => None,
            (Some((active_id, gen, _)), _) if *active_id == build.id => return Ok(()),
            (Some((active_id, gen, _)), Some(pred))
                if *pred == *active_id
                    && i64::try_from(build.generation).map_err(|_| {
                        StoreError::Invariant("generation overflows i64".into())
                    })? > *gen =>
            {
                Some((pred.clone(), *gen))
            }
            (Some((active_id, gen, _)), None) => {
                return Err(StoreError::Invariant(format!(
                    "predecessor mismatch: expected None, live active is {active_id} (gen {gen})"
                )));
            }
            (Some((active_id, gen, _)), Some(pred)) => {
                let build_gen = i64::try_from(build.generation)
                    .map_err(|_| StoreError::Invariant("generation overflows i64".into()))?;
                if *pred == *active_id && build_gen <= *gen {
                    return Err(StoreError::Invariant(
                        "older worker cannot replace newer build".into(),
                    ));
                }
                return Err(StoreError::Invariant(format!(
                    "predecessor mismatch: expected {pred:?}, live active is {active_id} (gen {gen})"
                )));
            }
            (None, Some(pred)) => {
                return Err(StoreError::Invariant(format!(
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
            .map_err(|_| StoreError::Invariant("generation overflows i64".into()))?;
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
