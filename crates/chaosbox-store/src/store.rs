//! Storage abstraction: the [`Store`] trait and the in-memory backend.

use std::collections::BTreeMap;

use chaosbox_core::{Candidate, Claim, Decision, Entity, Evidence, GraphBuild, Relation, SnapshotFile};

use crate::StoreError;

/// Storage abstraction: implemented by the `TypeDB` backend
/// (`chaosbox-typedb`) over the live server, or by [`MemoryStore`] for
/// tests and environments without a server.
/// Mutating methods are async because the live backend needs network IO;
/// file versions must be registered with [`Store::ensure_snapshot_files`]
/// before evidence referencing them is stored; hashes are never invented.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// Register one snapshot's file content identities (idempotent).
    /// Must precede any [`Store::put_evidence`] for those files.
    async fn ensure_snapshot_files(
        &mut self,
        snapshot_id: &str,
        repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), StoreError>;
    /// Register one extraction run + candidate set (idempotent).
    /// A set id with a different catalog digest or rubric is rejected:
    /// set identity covers its inputs.
    async fn ensure_run(
        &mut self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
        set_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), StoreError>;
    /// Record a candidate of a registered set (idempotent per candidate id).
    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), StoreError>;
    /// Stage an entity (idempotent); validated at publication.
    async fn put_entity(&mut self, e: Entity) -> Result<(), StoreError>;
    /// Stage a relationship for one build (idempotent).
    async fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), StoreError>;
    /// Record a decision (idempotent per candidate + question while inputs
    /// are unchanged; replaced when the cache key differs or a recorded
    /// `Failed` decision is retried).
    async fn put_decision(&mut self, d: Decision) -> Result<(), StoreError>;
    /// Record evidence (idempotent per evidence id; the (snapshot, path)
    /// file version must be registered first).
    async fn put_evidence(&mut self, e: Evidence) -> Result<(), StoreError>;
    /// Record a claim (idempotent per claim id).
    async fn put_claim(&mut self, c: Claim) -> Result<(), StoreError>;
    /// Look up a stored decision by candidate + question for cache reuse.
    /// Returns `None` on a miss; callers compare `cache_key` themselves.
    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, StoreError>;
    /// Validate invariants and atomically swing the active-build pointer.
    /// Rejects stale predecessors and older-worker overwrites.
    async fn publish(
        &mut self,
        build: GraphBuild,
        expected_predecessor: Option<String>,
    ) -> Result<(), StoreError>;
    /// The active (last good) build for a repository, if any.
    fn active(&self, repo: &str) -> Option<GraphBuild>;
    /// A build by id, active or superseded.
    fn get(&self, build_id: &str) -> Option<GraphBuild>;
    /// Stored-record counts for tests and operator diagnostics.
    fn stats(&self) -> StoreStats;
}

/// In-memory store: same invariants as the live path (registered file
/// versions, idempotent writes, predecessor-checked publication, immutable
/// published builds via clone).
#[derive(Default)]
pub struct MemoryStore {
    pub(crate) builds: BTreeMap<String, GraphBuild>,
    pub(crate) active: BTreeMap<String, String>,
    pub(crate) decisions: BTreeMap<String, Decision>,
    pub(crate) evidence: BTreeMap<String, Evidence>,
    pub(crate) claims: BTreeMap<String, Claim>,
    /// (snapshot, path) -> (sha256, bytes); evidence linkage validated here.
    pub(crate) files: BTreeMap<(String, String), (String, u64)>,
    /// run id -> (repo, snapshot id).
    pub(crate) runs: BTreeMap<String, (String, String)>,
    /// set id -> (run id, catalog digest, rubric version).
    pub(crate) sets: BTreeMap<String, (String, String, String)>,
    /// candidate id -> (set id, candidate).
    pub(crate) candidates: BTreeMap<String, (String, Candidate)>,
}

/// Staged write-ahead contents for backends that flush at publication
/// (`TypeDB` port): everything the staging [`MemoryStore`] validated, in
/// deterministic key order, ready for idempotent row inserts.
#[derive(Clone, Debug, Default)]
pub struct StagedData {
    /// run id -> (repo, snapshot id).
    pub runs: Vec<(String, (String, String))>,
    /// set id -> (run id, catalog digest, rubric version).
    pub sets: Vec<(String, (String, String, String))>,
    /// candidate id -> (set id, candidate).
    pub candidates: Vec<(String, (String, Candidate))>,
    /// Decisions keyed by (candidate, question), in key order.
    pub decisions: Vec<Decision>,
    /// Evidence by id, in id order.
    pub evidence: Vec<Evidence>,
    /// Claims by id, in id order.
    pub claims: Vec<Claim>,
    /// (snapshot, path) -> (sha256, bytes).
    pub files: Vec<((String, String), (String, u64))>,
}

impl MemoryStore {
    /// Export the staged contents for a publication flush. Ordering is
    /// deterministic (`BTreeMap` key order) so retries replay identically.
    #[must_use]
    pub fn export_staged(&self) -> StagedData {
        StagedData {
            runs: self
                .runs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            sets: self
                .sets
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            candidates: self
                .candidates
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            decisions: self.decisions.values().cloned().collect(),
            evidence: self.evidence.values().cloned().collect(),
            claims: self.claims.values().cloned().collect(),
            files: self
                .files
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// An empty store with no builds and no active pointers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stored-record counts (decisions, evidence, claims, builds) for
    /// pipeline tests and operator diagnostics.
    #[must_use]
    pub fn stats(&self) -> StoreStats {
        StoreStats {
            decisions: self.decisions.len(),
            evidence: self.evidence.len(),
            claims: self.claims.len(),
            builds: self.builds.len(),
            candidates: self.candidates.len(),
            runs: self.runs.len(),
        }
    }
}

/// Stored-record counts; see [`MemoryStore::stats`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreStats {
    /// Recorded decisions.
    pub decisions: usize,
    /// Recorded evidence rows.
    pub evidence: usize,
    /// Recorded claims.
    pub claims: usize,
    /// Published builds.
    pub builds: usize,
    /// Recorded candidates.
    pub candidates: usize,
    /// Registered runs.
    pub runs: usize,
}

#[async_trait::async_trait]
impl Store for MemoryStore {
    async fn ensure_snapshot_files(
        &mut self,
        snapshot_id: &str,
        _repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), StoreError> {
        for f in files {
            if f.snapshot != snapshot_id {
                return Err(StoreError::Invariant(format!(
                    "file {} lists snapshot {}, registered under {}",
                    f.path, f.snapshot, snapshot_id
                )));
            }
            self.files.insert(
                (snapshot_id.to_owned(), f.path.clone()),
                (f.sha256.clone(), f.bytes),
            );
        }
        Ok(())
    }

    async fn put_entity(&mut self, _e: Entity) -> Result<(), StoreError> {
        Ok(()) // entities live inside builds; staging validated at publish
    }
    async fn put_relation(&mut self, _r: Relation, _build: &str) -> Result<(), StoreError> {
        Ok(())
    }
    async fn put_decision(&mut self, d: Decision) -> Result<(), StoreError> {
        // Idempotent per (candidate, question) while inputs are unchanged.
        // A different cache key means the inputs changed (model, rubric,
        // catalog, questions, source): the stale row is replaced. A recorded
        // Failed decision is superseded even under the same key (retry).
        // Anything else keeps the first write.
        let key = format!("{}:{}", d.candidate_id, d.question_id);
        let replace = match self.decisions.get(&key) {
            None => true,
            Some(old) => {
                old.cache_key != d.cache_key
                    || matches!(old.outcome, chaosbox_core::DecisionOutcome::Failed(_))
            }
        };
        if replace {
            self.decisions.insert(key, d);
        }
        Ok(())
    }
    async fn put_evidence(&mut self, e: Evidence) -> Result<(), StoreError> {
        if !self
            .files
            .contains_key(&(e.snapshot.clone(), e.source_file_version.clone()))
        {
            return Err(StoreError::Invariant(format!(
                "evidence {} references unregistered file {} in snapshot {}",
                e.id, e.source_file_version, e.snapshot
            )));
        }
        self.evidence.entry(e.id.clone()).or_insert(e);
        Ok(())
    }
    async fn put_claim(&mut self, c: Claim) -> Result<(), StoreError> {
        self.claims.entry(c.id.clone()).or_insert(c);
        Ok(())
    }

    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, StoreError> {
        Ok(self
            .decisions
            .get(&format!("{candidate_id}:{question_id}"))
            .cloned())
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
        self.runs
            .entry(run_id.to_owned())
            .or_insert_with(|| (repo.to_owned(), snapshot_id.to_owned()));
        if let Some((run, catalog, rubric)) = self.sets.get(set_id) {
            if run != run_id || catalog != catalog_digest || rubric != rubric_version {
                return Err(StoreError::Invariant(format!(
                    "candidate set {set_id} already registered with different inputs"
                )));
            }
            return Ok(());
        }
        self.sets.insert(
            set_id.to_owned(),
            (
                run_id.to_owned(),
                catalog_digest.to_owned(),
                rubric_version.to_owned(),
            ),
        );
        Ok(())
    }

    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), StoreError> {
        if !self.sets.contains_key(set_id) {
            return Err(StoreError::Invariant(format!(
                "candidate {} references unregistered set {set_id}",
                c.id
            )));
        }
        self.candidates
            .entry(c.id.clone())
            .or_insert_with(|| (set_id.to_owned(), c.clone()));
        Ok(())
    }
    async fn publish(
        &mut self,
        build: GraphBuild,
        expected_predecessor: Option<String>,
    ) -> Result<(), StoreError> {
        // Validate invariants before pointer swing.
        for r in build.edges.values() {
            if !build.nodes.contains_key(&r.from) || !build.nodes.contains_key(&r.to) {
                return Err(StoreError::Invariant(format!(
                    "edge {} outside build {}",
                    r.id, build.id
                )));
            }
        }
        if let Some(cur_id) = self.active.get(&build.repo) {
            let cur = self
                .builds
                .get(cur_id)
                .ok_or_else(|| StoreError::NotFound(cur_id.clone()))?;
            if expected_predecessor.as_deref() != Some(&cur.id) {
                return Err(StoreError::Invariant(format!(
                    "predecessor mismatch: expected {:?}, active is {} (gen {})",
                    expected_predecessor, cur.id, cur.generation
                )));
            }
            if build.generation <= cur.generation {
                return Err(StoreError::Invariant(
                    "older worker cannot replace newer build".into(),
                ));
            }
        } else if expected_predecessor.is_some() {
            return Err(StoreError::Invariant(
                "expected predecessor but no active build".into(),
            ));
        }
        self.builds.insert(build.id.clone(), build.clone());
        self.active.insert(build.repo.clone(), build.id.clone());
        Ok(())
    }
    fn active(&self, repo: &str) -> Option<GraphBuild> {
        self.active
            .get(repo)
            .and_then(|id| self.builds.get(id))
            .cloned()
    }
    fn get(&self, build_id: &str) -> Option<GraphBuild> {
        self.builds.get(build_id).cloned()
    }
    fn stats(&self) -> StoreStats {
        self.stats()
    }
}
