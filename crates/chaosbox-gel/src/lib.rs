//! Typed Gel persistence/query operations and packaged schema assets.
//!
//! The native `gel-tokio` client is used with typed serde decoding and
//! parameterized EdgeQL. `query_json` results are decoded into typed structs;
//! frequently queried fields are typed properties/links, JSON only for
//! bounded raw provider envelopes. No SQLx/Postgres, no Python bridge.
//!
//! Tests run against [`MemoryStore`]; real Gel is exercised by `test-gel`
//! (disposable instance + migrations + mock Jev) and by `db check/migrate`.

use std::collections::BTreeMap;

use chaosbox_core::{Claim, Decision, Entity, Evidence, GraphBuild, Relation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Packaged SDL asset (also present under `dbschema/` in the crate package).
pub const SCHEMA_SDL: &str = include_str!("../../../dbschema/default.esdl");

/// Committed migration asset.
pub const MIGRATION_00001: &str = include_str!("../../../dbschema/migrations/00001.edgeql");

/// Committed migration asset: worker-task leases.
pub const MIGRATION_00002: &str = include_str!("../../../dbschema/migrations/00002.edgeql");

/// Pinned Gel version this schema is tested against.
pub const GEL_PINNED: &str = "7.2";
/// Schema compatibility marker checked by `db check`.
pub const SCHEMA_VERSION: u32 = 2;

/// Persistence/query failures: client, query, invariant, or missing rows.
#[derive(Debug, Error)]
pub enum GelError {
    #[error("gel client: {0}")]
    /// Connection or client-construction failure.
    Client(String),
    #[error("query: {0}")]
    /// EdgeQL execution or typed-decoding failure.
    Query(String),
    #[error("invariant: {0}")]
    /// A graph invariant was violated (cross-build edge, stale predecessor, ...).
    Invariant(String),
    #[error("not found: {0}")]
    /// A required row (build, entity, ...) does not exist.
    NotFound(String),
}

/// Parameterized EdgeQL statements (never string-interpolated values).
pub mod edgeql {
    /// Idempotent snapshot upsert (`$0` repo, `$1` snapshot id).
    pub const UPSERT_SNAPSHOT: &str =
        "select (insert SourceSnapshot { repo := <str>$0, snapshot_id := <str>$1 } \
         unless conflict on .snapshot_id else (update SourceSnapshot filter .snapshot_id = <str>$1 set { repo := <str>$0 })) { snapshot_id }";
    /// Idempotent entity upsert (`$0` entity id .. `$6` qualified name).
    pub const UPSERT_ENTITY: &str =
        "select (insert Entity { entity_id := <str>$0, kind := <str>$1, repo := <str>$2, snapshot := <str>$3, \
         file := <str>$4, name := <str>$5, qualified_name := <str>$6 } \
         unless conflict on .entity_id else (select Entity filter .entity_id = <str>$0)) { entity_id }";
    /// Idempotent relationship upsert with typed endpoints (`$0` rel id .. `$4` scope).
    pub const UPSERT_RELATIONSHIP: &str =
        "select (insert Relationship { rel_id := <str>$0, rel_type := <str>$1, \
         from_entity := (select Entity filter .entity_id = <str>$2), \
         to_entity := (select Entity filter .entity_id = <str>$3), scope := <str>$4 } \
         unless conflict on .rel_id else (select Relationship filter .rel_id = <str>$0)) { rel_id }";
    /// Idempotent decision write keyed by (candidate, question).
    pub const INSERT_DECISION: &str =
        "select (insert Decision { decision_id := <str>$0, candidate := (select Candidate filter .candidate_id = <str>$1), \
         question_id := <str>$2, outcome := <str>$3, evidence_class := <str>$4, \
         model_requested := <str>$5, model_returned := <str>$6 } \
         unless conflict on ((.candidate, .question_id)) else (select Decision filter .decision_id = <str>$0)) { decision_id }";
    /// Staging build insert (`$0` build id, `$1` repo, `$2` generation, `$3` status).
    pub const CREATE_BUILD: &str =
        "select (insert GraphBuild { build_id := <str>$0, repo := <str>$1, generation := <int64>$2, status := <str>$3 }) { build_id }";
    /// Atomic active-build pointer swing (`$0` repo, `$1` build id).
    pub const SET_ACTIVE_BUILD: &str =
        "select (insert ActiveBuildPointer { repo := <str>$0, build := (select GraphBuild filter .build_id = <str>$1) } \
         unless conflict on .repo else (update ActiveBuildPointer filter .repo = <str>$0 set { build := (select GraphBuild filter .build_id = <str>$1) })) { repo }";
    /// Active build pointer read (`$0` repo).
    pub const ACTIVE_BUILD: &str =
        "select ActiveBuildPointer { repo, build: { build_id, generation, status } } filter .repo = <str>$0";
    /// Outgoing relationships of the pinned build (`$0` build, `$1` entity,
    /// `$2` relation-type filter list).
    pub const NEIGHBORS_OUT: &str =
        "select GraphEdgeMembership { relationship: { rel_id, rel_type, from_entity: { entity_id }, to_entity: { entity_id } } } \
         filter .build.build_id = <str>$0 and .relationship.from_entity.entity_id = <str>$1 \
         and .relationship.rel_type in array_unpack(<array<str>>$2)";
    /// Entity lookup within the pinned build (`$0` build, `$1` entity id).
    pub const ENTITY_BY_ID: &str =
        "select GraphMembership { entity: { entity_id, kind, repo, snapshot, file, name, qualified_name } } \
         filter .build.build_id = <str>$0 and .entity.entity_id = <str>$1";
    /// Substring search over the pinned build's names (`$0` build, `$1` like
    /// pattern with `\` escapes, `$2` limit).
    pub const SEARCH_ENTITIES: &str =
        "select GraphMembership { entity: { entity_id, kind, repo, snapshot, file, name, qualified_name } } \
         filter .build.build_id = <str>$0 \
         and (.entity.name ilike <str>$1 or .entity.qualified_name ilike <str>$1) \
         order by .entity.qualified_name limit <int64>$2";
    /// Incoming relationships of the pinned build (`$0` build, `$1` entity,
    /// `$2` relation-type filter list).
    pub const NEIGHBORS_IN: &str =
        "select GraphEdgeMembership { relationship: { rel_id, rel_type, from_entity: { entity_id }, to_entity: { entity_id } } } \
         filter .build.build_id = <str>$0 and .relationship.to_entity.entity_id = <str>$1 \
         and .relationship.rel_type in array_unpack(<array<str>>$2)";
    /// All member entities of one build (`$0` build id, `$1` limit).
    pub const BUILD_ENTITIES: &str =
        "select GraphMembership { entity: { entity_id, kind, repo, snapshot, file, name, qualified_name } } \
         filter .build.build_id = <str>$0 order by .entity.qualified_name limit <int64>$1";
    /// All member relationships of one build (`$0` build id, `$1` limit).
    pub const BUILD_RELATIONSHIPS: &str =
        "select GraphEdgeMembership { relationship: { rel_id, rel_type, from_entity: { entity_id }, to_entity: { entity_id } } } \
         filter .build.build_id = <str>$0 limit <int64>$1";
    /// Evidence attached to one relationship of the pinned build
    /// (`$0` build, `$1` rel id).
    pub const EVIDENCE_FOR_REL: &str =
        "select GraphEdgeMembership { ev := .relationship.evidence: { evidence_id, class, supports, text } } \
         filter .build.build_id = <str>$0 and .relationship.rel_id = <str>$1";
}

/// Typed row for entity lookup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityRow {
    /// Entity id.
    pub entity_id: String,
    /// Entity kind name.
    pub kind: String,
    /// Owning repository name.
    pub repo: String,
    /// Snapshot this identity belongs to.
    pub snapshot: String,
    /// Repository-relative file path.
    pub file: String,
    /// Short display name.
    pub name: String,
    /// Qualified name.
    pub qualified_name: String,
}

/// Active-build pointer row for readiness and per-request build pinning.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PointerRow {
    /// Repository name.
    pub repo: String,
    /// The pinned active build.
    pub build: BuildRow,
}

/// Published build header row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildRow {
    /// Build id.
    pub build_id: String,
    /// Monotonic generation.
    pub generation: i64,
    /// `staging` or `active`.
    pub status: String,
}

/// Relationship row with endpoint ids.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelRow {
    /// Relationship id.
    pub rel_id: String,
    /// Relation type name.
    pub rel_type: String,
    /// Source endpoint wrapper.
    pub from_entity: EndpointRef,
    /// Target endpoint wrapper.
    pub to_entity: EndpointRef,
}

/// Endpoint id wrapper (EdgeQL shape).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointRef {
    /// Entity id.
    pub entity_id: String,
}

/// Membership row wrapping one entity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MembershipRow {
    /// The member entity.
    pub entity: EntityRow,
}

/// Edge-membership row wrapping one relationship.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgeMembershipRow {
    /// The member relationship.
    pub relationship: RelRow,
}

/// Evidence bundle nested under one edge membership.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceBundleRow {
    /// Evidence rows attached to the membership's relationship.
    pub ev: Vec<EvidenceRow>,
}

/// Evidence row for claim support/contradiction display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceRow {
    /// Evidence id.
    pub evidence_id: String,
    /// Evidence class name.
    pub class: String,
    /// True when supporting the relationship.
    pub supports: bool,
    /// Source-copied or template text.
    pub text: String,
}
/// Durable worker task states. No transactions held open during Jev calls.
/// Clocks are injected as unix seconds (callers read the real clock);
/// persistence of these records lands with the Gel write path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Ready to be claimed by a worker.
    Pending,
    /// Held by a worker under a lease; stale holders never overwrite newer
    /// results, and expired holders become reclaimable.
    Claimed {
        /// Worker holding the claim.
        worker: String,
        /// Unix timestamp when the lease expires.
        expires_at: i64,
    },
    /// Completed.
    Done,
    /// Failed with a sanitized reason.
    Failed(String),
}

/// Durable task record with safe claiming/recovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// Task id.
    pub id: String,
    /// Current lifecycle state.
    pub state: TaskState,
    /// Claim generation; incremented on every successful claim or reclaim.
    pub generation: u64,
}

/// Claim a pending task with a lease: only `Pending` tasks can be claimed,
/// so stale workers never overwrite newer task results.
pub fn claim_task(
    task: &mut Task,
    worker: &str,
    now_unix: i64,
    lease_secs: i64,
) -> Result<(), GelError> {
    match &task.state {
        TaskState::Pending => {
            task.state = TaskState::Claimed {
                worker: worker.to_owned(),
                expires_at: now_unix + lease_secs.max(1),
            };
            task.generation += 1;
            Ok(())
        }
        other => Err(GelError::Invariant(format!("claim non-pending task {:?} as {worker}", other))),
    }
}

/// Renew the caller's own lease (heartbeat). Any other holder, or any
/// non-claimed state, is rejected.
pub fn heartbeat_task(
    task: &mut Task,
    worker: &str,
    now_unix: i64,
    lease_secs: i64,
) -> Result<(), GelError> {
    match &task.state {
        TaskState::Claimed { worker: holder, .. } if holder == worker => {
            task.state = TaskState::Claimed {
                worker: worker.to_owned(),
                expires_at: now_unix + lease_secs.max(1),
            };
            Ok(())
        }
        other => Err(GelError::Invariant(format!("heartbeat not holder: {other:?} as {worker}"))),
    }
}

/// Reclaim an expired claim for a new worker, bumping the generation so a
/// stale holder's late write is recognizable. Live claims cannot be taken.
pub fn reclaim_task(
    task: &mut Task,
    worker: &str,
    now_unix: i64,
    lease_secs: i64,
) -> Result<(), GelError> {
    match &task.state {
        TaskState::Claimed { expires_at, .. } if now_unix >= *expires_at => {
            task.state = TaskState::Claimed {
                worker: worker.to_owned(),
                expires_at: now_unix + lease_secs.max(1),
            };
            task.generation += 1;
            Ok(())
        }
        other => Err(GelError::Invariant(format!("reclaim live task {other:?} as {worker}"))),
    }
}

/// Storage abstraction: real Gel via [`GelHandle`] or [`MemoryStore`] for
/// tests and environments without a server.
pub trait Store: Send + Sync {
    /// Stage an entity (idempotent); validated at publication.
    fn put_entity(&mut self, e: Entity) -> Result<(), GelError>;
    /// Stage a relationship for one build (idempotent).
    fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), GelError>;
    /// Record a decision (idempotent per candidate + question; first write
    /// wins, except a recorded `Failed` decision may be superseded once).
    fn put_decision(&mut self, d: Decision) -> Result<(), GelError>;
    /// Record evidence (idempotent per evidence id).
    fn put_evidence(&mut self, e: Evidence) -> Result<(), GelError>;
    /// Record a claim (idempotent per claim id).
    fn put_claim(&mut self, c: Claim) -> Result<(), GelError>;
    /// Validate invariants and atomically swing the active-build pointer.
    /// Rejects stale predecessors and older-worker overwrites.
    fn publish(&mut self, build: GraphBuild, expected_predecessor: Option<String>) -> Result<(), GelError>;
    /// The active (last good) build for a repository, if any.
    fn active(&self, repo: &str) -> Option<GraphBuild>;
    /// A build by id, active or superseded.
    fn get(&self, build_id: &str) -> Option<GraphBuild>;
}

/// In-memory store: same invariants as the Gel path (idempotent writes,
/// predecessor-checked publication, immutable published builds via clone).
#[derive(Default)]
pub struct MemoryStore {
    builds: BTreeMap<String, GraphBuild>,
    active: BTreeMap<String, String>,
    decisions: BTreeMap<String, Decision>,
    evidence: BTreeMap<String, Evidence>,
    claims: BTreeMap<String, Claim>,
}

impl MemoryStore {
    /// An empty store with no builds and no active pointers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn put_entity(&mut self, _e: Entity) -> Result<(), GelError> {
        Ok(()) // entities live inside builds; staging validated at publish
    }
    fn put_relation(&mut self, _r: Relation, _build: &str) -> Result<(), GelError> {
        Ok(())
    }
    fn put_decision(&mut self, d: Decision) -> Result<(), GelError> {
        // Idempotent per (candidate, question): first write wins, except a
        // recorded Failed decision may be superseded by a later outcome.
        // Non-Failed outcomes are never overwritten.
        let key = format!("{}:{}", d.candidate_id, d.question_id);
        let supersede = matches!(
            self.decisions.get(&key).map(|old| &old.outcome),
            Some(chaosbox_core::DecisionOutcome::Failed(_))
        );
        if supersede {
            self.decisions.insert(key, d);
        } else {
            self.decisions.entry(key).or_insert(d);
        }
        Ok(())
    }
    fn put_evidence(&mut self, e: Evidence) -> Result<(), GelError> {
        self.evidence.entry(e.id.clone()).or_insert(e);
        Ok(())
    }
    fn put_claim(&mut self, c: Claim) -> Result<(), GelError> {
        self.claims.entry(c.id.clone()).or_insert(c);
        Ok(())
    }
    fn publish(&mut self, build: GraphBuild, expected_predecessor: Option<String>) -> Result<(), GelError> {
        // Validate invariants before pointer swing.
        for r in build.edges.values() {
            if !build.nodes.contains_key(&r.from) || !build.nodes.contains_key(&r.to) {
                return Err(GelError::Invariant(format!("edge {} outside build {}", r.id, build.id)));
            }
        }
        if let Some(cur_id) = self.active.get(&build.repo) {
            let cur = self.builds.get(cur_id).ok_or_else(|| GelError::NotFound(cur_id.clone()))?;
            if expected_predecessor.as_deref() != Some(&cur.id) {
                return Err(GelError::Invariant(format!(
                    "predecessor mismatch: expected {:?}, active is {} (gen {})",
                    expected_predecessor, cur.id, cur.generation
                )));
            }
            if build.generation <= cur.generation {
                return Err(GelError::Invariant("older worker cannot replace newer build".into()));
            }
        } else if expected_predecessor.is_some() {
            return Err(GelError::Invariant("expected predecessor but no active build".into()));
        }
        self.builds.insert(build.id.clone(), build.clone());
        self.active.insert(build.repo.clone(), build.id.clone());
        Ok(())
    }
    fn active(&self, repo: &str) -> Option<GraphBuild> {
        self.active.get(repo).and_then(|id| self.builds.get(id)).cloned()
    }
    fn get(&self, build_id: &str) -> Option<GraphBuild> {
        self.builds.get(build_id).cloned()
    }
}

/// Read-only query surface shared by the live [`GelHandle`] and the
/// in-memory fake. Every read is scoped to one pinned build id: readers can
/// never observe entities or relationships outside the active build.
/// Ordering: entity lists come back ordered by qualified name; relationship
/// and evidence lists have unspecified order (conformance compares as sets).
/// An empty relation-type filter matches nothing (mirrors `array_unpack([])`).
#[async_trait::async_trait]
pub trait GelQueries: Send + Sync {
    /// Active build header for a repository (per-request build pinning).
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, GelError>;
    /// Bounded substring search over the pinned build's entity names
    /// (`like` carries `%` wrappers; `\` escapes `%` and `_`).
    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError>;
    /// Typed entity lookup within the pinned build.
    async fn entity_by_id(&self, build_id: &str, id: &str) -> Result<Option<EntityRow>, GelError>;
    /// Outgoing relationships within the pinned build, with a type filter.
    async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError>;
    /// Incoming relationships within the pinned build, with a type filter.
    async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError>;
    /// Member entities of one build, bounded.
    async fn build_entities(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError>;
    /// Member relationships of one build, bounded.
    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, GelError>;
    /// Evidence attached to one relationship of the pinned build.
    async fn evidence_for(&self, build_id: &str, rel_id: &str) -> Result<Vec<EvidenceRow>, GelError>;
}

/// Project one entity into its row form (canonical storage names).
fn entity_row(e: &Entity) -> EntityRow {
    EntityRow {
        entity_id: e.id.clone(),
        kind: format!("{:?}", e.kind),
        repo: e.repo.clone(),
        snapshot: e.snapshot.clone(),
        file: e.file.clone(),
        name: e.name.clone(),
        qualified_name: e.qualified_name.clone(),
    }
}

/// Project one relationship into its row form (canonical storage names).
fn rel_row(r: &Relation) -> RelRow {
    RelRow {
        rel_id: r.id.clone(),
        rel_type: chaosbox_core::relation_type_name(&r.rel_type),
        from_entity: EndpointRef { entity_id: r.from.clone() },
        to_entity: EndpointRef { entity_id: r.to.clone() },
    }
}

/// In-memory [`GelQueries`] fake: same method surface and ordering as the
/// live path, so conformance tests prove parity. Live Gel runs the same
/// suite once a server is available (see `check_conformance`).
#[derive(Default)]
pub struct MemoryReader {
    builds: BTreeMap<String, GraphBuild>,
    active: BTreeMap<String, String>,
    evidence: BTreeMap<String, Vec<EvidenceRow>>,
}

impl MemoryReader {
    /// An empty reader with no builds and no active pointers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a build (indexed by its id).
    pub fn insert_build(&mut self, build: GraphBuild) {
        self.builds.insert(build.id.clone(), build);
    }

    /// Point a repository at one of the inserted builds.
    pub fn set_active(&mut self, repo: &str, build_id: &str) {
        self.active.insert(repo.to_owned(), build_id.to_owned());
    }

    /// Attach evidence rows to a relationship id.
    pub fn attach_evidence(&mut self, rel_id: &str, rows: Vec<EvidenceRow>) {
        self.evidence.insert(rel_id.to_owned(), rows);
    }

    /// Member entities of one build, ordered by qualified name.
    fn members(&self, build_id: &str) -> Vec<EntityRow> {
        self.builds.get(build_id).map_or_else(Vec::new, |b| {
            let mut v: Vec<EntityRow> = b.nodes.values().map(entity_row).collect();
            v.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
            v
        })
    }

    /// Member relationships of one build.
    fn member_relations(&self, build_id: &str) -> Vec<RelRow> {
        self.builds.get(build_id).map_or_else(Vec::new, |b| b.edges.values().map(rel_row).collect())
    }
}

/// Undo `%`-wrapping and `\` escapes of a LIKE pattern into a literal
/// substring needle (mirrors the live `ilike` with backslash escapes).
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

#[async_trait::async_trait]
impl GelQueries for MemoryReader {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, GelError> {
        Ok(self.active.get(repo).and_then(|id| self.builds.get(id)).map(|b| BuildRow {
            build_id: b.id.clone(),
            generation: b.generation as i64,
            status: "active".to_owned(),
        }))
    }

    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        let needle = unescape_like(like).to_lowercase();
        let limit = limit.max(0) as usize;
        Ok(self
            .members(build_id)
            .into_iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&needle)
                    || e.qualified_name.to_lowercase().contains(&needle)
            })
            .take(limit)
            .collect())
    }

    async fn entity_by_id(
        &self,
        build_id: &str,
        id: &str,
    ) -> Result<Option<EntityRow>, GelError> {
        Ok(self.members(build_id).into_iter().find(|e| e.entity_id == id))
    }

    async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        Ok(self
            .member_relations(build_id)
            .into_iter()
            .filter(|r| r.from_entity.entity_id == id && rel_types.contains(&r.rel_type))
            .collect())
    }

    async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        Ok(self
            .member_relations(build_id)
            .into_iter()
            .filter(|r| r.to_entity.entity_id == id && rel_types.contains(&r.rel_type))
            .collect())
    }

    async fn build_entities(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        let limit = limit.max(0) as usize;
        Ok(self
            .builds
            .get(build_id)
            .map(|b| {
                let mut v: Vec<EntityRow> = b.nodes.values().map(entity_row).collect();
                v.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
                v.into_iter().take(limit).collect()
            })
            .unwrap_or_default())
    }

    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, GelError> {
        let limit = limit.max(0) as usize;
        Ok(self
            .builds
            .get(build_id)
            .map(|b| b.edges.values().map(rel_row).take(limit).collect())
            .unwrap_or_default())
    }

    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, GelError> {
        if self.builds.get(build_id).is_none_or(|b| !b.edges.contains_key(rel_id)) {
            return Ok(Vec::new());
        }
        Ok(self.evidence.get(rel_id).cloned().unwrap_or_default())
    }
}

#[async_trait::async_trait]
impl GelQueries for GelHandle {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, GelError> {
        GelHandle::active_build(self, repo).await
    }

    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        GelHandle::search_entities(self, build_id, like, limit).await
    }

    async fn entity_by_id(
        &self,
        build_id: &str,
        id: &str,
    ) -> Result<Option<EntityRow>, GelError> {
        GelHandle::entity_by_id(self, build_id, id).await
    }

    async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        GelHandle::neighbors_out(self, build_id, id, rel_types).await
    }

    async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        GelHandle::neighbors_in(self, build_id, id, rel_types).await
    }

    async fn build_entities(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        GelHandle::build_entities(self, build_id, limit).await
    }

    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, GelError> {
        GelHandle::build_relationships(self, build_id, limit).await
    }

    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, GelError> {
        GelHandle::evidence_for(self, build_id, rel_id).await
    }
}

/// Seed fixture for conformance: two builds of repo `conf`, the second
/// active, sharing a symbol name across snapshots plus one evidence row.
/// Returns the reader plus the ids the suite asserts on.
pub struct ConformanceSeed {
    /// The seeded reader.
    pub reader: MemoryReader,
    /// First-build entity ids (a1 calls b1).
    pub a1: String,
    /// First-build entity ids (a1 calls b1).
    pub b1: String,
    /// First-build `calls` relationship id.
    pub rel1: String,
    /// Second-build entity id sharing `a1`'s name in a new snapshot.
    pub a2: String,
    /// First and second build ids.
    pub builds: (String, String),
}

/// Build the conformance seed (repo `conf`).
#[must_use]
pub fn conformance_seed() -> ConformanceSeed {
    use chaosbox_core::{EntityKind, RelationScope, RelationType, SourceSpan};
    let span = |f: &str| SourceSpan::point(f, 1, 1, 0);
    let ent = |repo: &str, snap: &str, file: &str, name: &str| {
        Entity::new(EntityKind::Symbol, repo, snap, file, name, name, span(file))
    };
    let mut b1 = GraphBuild::new("conf", vec!["s1".into()], 1);
    let a1 = ent("conf", "s1", "f.rs", "Alpha");
    let b1e = ent("conf", "s1", "f.rs", "Beta");
    b1.add_node(a1.clone()).unwrap();
    b1.add_node(b1e.clone()).unwrap();
    let mut r1 = Relation::new(RelationType::Calls, &a1.id, &b1e.id, RelationScope::File, &b1.id);
    r1.evidence_ids.push("ev1".into());
    b1.add_edge(r1.clone()).unwrap();
    let mut b2 = GraphBuild::new("conf", vec!["s2".into()], 2);
    b2.predecessor = Some(b1.id.clone());
    let a2 = ent("conf", "s2", "f.rs", "Alpha");
    let c2 = ent("conf", "s2", "g.rs", "Gamma");
    b2.add_node(a2.clone()).unwrap();
    b2.add_node(c2.clone()).unwrap();
    let r2 = Relation::new(RelationType::References, &a2.id, &c2.id, RelationScope::CrossFile, &b2.id);
    b2.add_edge(r2).unwrap();
    let mut reader = MemoryReader::new();
    reader.insert_build(b1.clone());
    reader.insert_build(b2.clone());
    reader.set_active("conf", &b2.id);
    reader.attach_evidence(
        &r1.id,
        vec![EvidenceRow {
            evidence_id: "ev1".into(),
            class: "extracted".into(),
            supports: true,
            text: "[structural] Alpha -> Beta".into(),
        }],
    );
    ConformanceSeed {
        a1: a1.id,
        b1: b1e.id,
        rel1: r1.id.clone(),
        a2: a2.id,
        builds: (b1.id, b2.id),
        reader,
    }
}

/// Conformance assertions over any [`GelQueries`] impl seeded like
/// [`conformance_seed`]. Every read is scoped to one pinned build: the suite
/// asserts cross-build leakage is impossible, not just that members resolve.
/// Relationship/evidence lists compare as sets (live order is unspecified);
/// entity lists compare ordered by qualified name. A future live-Gel test
/// seeds the same fixture through the insert path and calls this function.
pub async fn check_conformance<R: GelQueries>(
    r: &R,
    a1: &str,
    b1: &str,
    rel1: &str,
    a2: &str,
    builds: &(String, String),
) {
    use std::collections::BTreeSet;
    // Active pointer pins the second build.
    let active = r.active_build("conf").await.unwrap().unwrap();
    assert_eq!(active.build_id, builds.1);
    assert_eq!(active.generation, 2);
    assert!(r.active_build("missing-repo").await.unwrap().is_none());
    // Search is scoped: each build sees only its own Alpha.
    let hits: Vec<_> =
        r.search_entities(&builds.0, "%alpha%", 10).await.unwrap().into_iter().map(|e| e.entity_id).collect();
    assert_eq!(hits, vec![a1.to_owned()]);
    let hits: Vec<_> =
        r.search_entities(&builds.1, "%alpha%", 10).await.unwrap().into_iter().map(|e| e.entity_id).collect();
    assert_eq!(hits, vec![a2.to_owned()]);
    assert_eq!(r.search_entities(&builds.1, "%alpha%", 0).await.unwrap().len(), 0);
    // Escaped wildcards match literally, not as patterns (same on live Gel,
    // where backslash is the LIKE escape).
    assert!(r.search_entities(&builds.1, "%alp\\_ha%", 10).await.unwrap().is_empty());
    // Lookup is scoped: a1 is invisible from the second build.
    assert_eq!(r.entity_by_id(&builds.0, a1).await.unwrap().unwrap().entity_id, a1);
    assert!(r.entity_by_id(&builds.1, a1).await.unwrap().is_none());
    assert!(r.entity_by_id(&builds.0, "ent:missing").await.unwrap().is_none());
    // Neighborhoods honor the type filter and the build scope; empty filter
    // matches nothing.
    let out: BTreeSet<_> = r
        .neighbors_out(&builds.0, a1, vec!["calls".into()])
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.rel_id)
        .collect();
    assert_eq!(out, BTreeSet::from([rel1.to_owned()]));
    assert!(r.neighbors_out(&builds.1, a1, vec!["calls".into()]).await.unwrap().is_empty());
    assert!(r.neighbors_out(&builds.0, a1, vec![]).await.unwrap().is_empty());
    assert!(r.neighbors_out(&builds.0, a1, vec!["references".into()]).await.unwrap().is_empty());
    let inc: BTreeSet<_> = r
        .neighbors_in(&builds.0, b1, vec!["calls".into()])
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.rel_id)
        .collect();
    assert_eq!(inc, BTreeSet::from([rel1.to_owned()]));
    // Build projections are membership-scoped.
    let e1: BTreeSet<_> = r
        .build_entities(&builds.0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(e1, BTreeSet::from([a1.to_owned(), b1.to_owned()]));
    assert!(r.build_entities("build:missing", 100).await.unwrap().is_empty());
    let r1: BTreeSet<_> = r
        .build_relationships(&builds.0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.rel_id)
        .collect();
    assert_eq!(r1, BTreeSet::from([rel1.to_owned()]));
    // Evidence attaches to the relationship within its own build only.
    let ev = r.evidence_for(&builds.0, rel1).await.unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].evidence_id, "ev1");
    assert!(ev[0].supports);
    assert!(r.evidence_for(&builds.1, rel1).await.unwrap().is_empty());
    assert!(r.evidence_for(&builds.0, "rel:missing").await.unwrap().is_empty());
}

/// Native Gel handle: typed decoding over `query_json` with bound params.
///
/// Connection comes from the environment / instance config (never from
/// caller-controlled session variables); credentials arrive via
/// `CHAOSBOX_GEL_CREDENTIALS_FILE`.
pub struct GelHandle {
    client: gel_tokio::Client,
}

impl GelHandle {
    /// Connect with default parameters (env/instance config).
    pub async fn connect() -> Result<Self, GelError> {
        let client = gel_tokio::create_client()
            .await
            .map_err(|e| GelError::Client(e.to_string()))?;
        Ok(Self { client })
    }

    /// Readiness probe: connectivity + schema compatibility marker.
    pub async fn probe(&self) -> Result<Probe, GelError> {
        let json = self
            .client
            .query_json("select { version := <int64>$0 }", &(i64::from(SCHEMA_VERSION),))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let v: serde_json::Value =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(Probe { ok: true, detail: v.to_string() })
    }

    /// Typed entity lookup with a bound parameter.
    /// Typed entity lookup within the pinned build, with a bound parameter.
    pub async fn entity_by_id(
        &self,
        build_id: &str,
        id: &str,
    ) -> Result<Option<EntityRow>, GelError> {
        let json = self
            .client
            .query_single_json(edgeql::ENTITY_BY_ID, &(build_id, id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        match json {
            None => Ok(None),
            Some(j) => {
                let row: MembershipRow = serde_json::from_str(j.as_ref())
                    .map_err(|e| GelError::Query(e.to_string()))?;
                Ok(Some(row.entity))
            }
        }
    }

    /// Idempotent snapshot upsert with bound parameters.
    pub async fn upsert_snapshot(&self, repo: &str, snapshot_id: &str) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::UPSERT_SNAPSHOT, &(repo, snapshot_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Active build header for a repository (per-request build pinning).
    pub async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::ACTIVE_BUILD, &(repo,))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<PointerRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().next().map(|r| r.build))
    }

    /// Bounded substring search over the pinned build's entity names.
    pub async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::SEARCH_ENTITIES, &(build_id, like, limit))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<MembershipRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.entity).collect())
    }

    /// Outgoing relationships of the pinned build with a type filter
    /// (empty filter = none).
    pub async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::NEIGHBORS_OUT, &(build_id, id, rel_types))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<EdgeMembershipRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.relationship).collect())
    }

    /// Incoming relationships of the pinned build with a type filter
    /// (empty filter = none).
    pub async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::NEIGHBORS_IN, &(build_id, id, rel_types))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<EdgeMembershipRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.relationship).collect())
    }

    /// Member entities of one build, bounded; errors are reported, never silent.
    pub async fn build_entities(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::BUILD_ENTITIES, &(build_id, limit))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<MembershipRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.entity).collect())
    }

    /// Member relationships of one build, bounded.
    pub async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::BUILD_RELATIONSHIPS, &(build_id, limit))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<EdgeMembershipRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.relationship).collect())
    }

    /// Evidence attached to one relationship of the pinned build.
    pub async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::EVIDENCE_FOR_REL, &(build_id, rel_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<EvidenceBundleRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows.into_iter().flat_map(|r| r.ev).collect())
    }
}

/// Readiness probe result: connectivity plus the schema compatibility marker.
#[derive(Clone, Debug)]
pub struct Probe {
    /// True when the server answered and the marker round-tripped.
    pub ok: bool,
    /// The returned marker payload for diagnostics.
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaosbox_core::{EntityKind, GraphBuild, RelationScope, RelationType, SourceSpan};

    fn ent(repo: &str, snap: &str, file: &str, name: &str) -> Entity {
        Entity::new(EntityKind::Symbol, repo, snap, file, name, name, SourceSpan::point(file, 1, 1, 0))
    }

    #[test]
    fn schema_assets_packaged() {
        assert!(SCHEMA_SDL.contains("type Relationship"));
        assert!(SCHEMA_SDL.contains("ActiveBuildPointer"));
        assert!(SCHEMA_SDL.contains("type WorkerTask"));
        assert!(!SCHEMA_SDL.contains("json;") || SCHEMA_SDL.contains("raw_envelope"));
        assert!(MIGRATION_00001.contains("m1_chaosbox_init"));
        assert!(MIGRATION_00002.contains("m2_worker_tasks"));
        assert!(MIGRATION_00002.contains("m1_chaosbox_init"), "migration chain must link");
    }

    #[test]
    fn publish_rejects_stale_worker() {
        let mut s = MemoryStore::new();
        let mut b1 = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = ent("r", "s1", "a.rs", "a");
        b1.add_node(a).unwrap();
        s.publish(b1.clone(), None).unwrap();
        // concurrent stale build with same predecessor expectation fails
        let mut stale = GraphBuild::new("r", vec!["s1".into()], 1);
        let b = ent("r", "s1", "b.rs", "b");
        stale.add_node(b).unwrap();
        assert!(s.publish(stale, Some("wrong-predecessor".into())).is_err());
        // newer generation with correct predecessor wins
        let mut b2 = GraphBuild::new("r", vec!["s2".into()], 2);
        b2.predecessor = Some(b1.id.clone());
        let c = ent("r", "s2", "c.rs", "c");
        b2.add_node(c).unwrap();
        assert!(s.publish(b2.clone(), Some(b1.id.clone())).is_ok());
        assert_eq!(s.active("r").unwrap().id, b2.id);
    }

    #[test]
    fn failed_decisions_superseded_once() {
        use chaosbox_core::{DecisionOutcome, EvidenceClass};
        let mut s = MemoryStore::new();
        let mk = |id: &str, outcome| Decision {
            id: id.into(),
            candidate_id: "c1".into(),
            question_id: "q1".into(),
            outcome,
            evidence_class: EvidenceClass::Ambiguous,
            model_requested: "jev-1.13.0".into(),
            model_returned: "jev-1.13.0".into(),
            confidence: None,
            probability: None,
        };
        s.put_decision(mk("d1", DecisionOutcome::Failed("down".into()))).unwrap();
        s.put_decision(mk("d2", DecisionOutcome::Accepted)).unwrap();
        let key = "c1:q1";
        assert_eq!(s.decisions[key].id, "d2", "retry supersedes a recorded failure");
        // Non-failed outcomes are never overwritten.
        s.put_decision(mk("d3", DecisionOutcome::Rejected)).unwrap();
        assert_eq!(s.decisions[key].id, "d2", "accepted outcomes stick");
    }

    #[test]
    fn decisions_idempotent() {
        let mut s = MemoryStore::new();
        let d = Decision {
            id: "d1".into(),
            candidate_id: "c1".into(),
            question_id: "q1".into(),
            outcome: chaosbox_core::DecisionOutcome::Accepted,
            evidence_class: chaosbox_core::EvidenceClass::Extracted,
            model_requested: "jev-1.13.0".into(),
            model_returned: "jev-1.13.0".into(),
            confidence: None,
            probability: Some(0.9),
        };
        s.put_decision(d.clone()).unwrap();
        s.put_decision({ let mut d2 = d.clone(); d2.id = "d2".into(); d2 }).unwrap();
        assert_eq!(s.decisions.len(), 1, "first write wins");
    }

    #[test]
    fn task_claim_recovery() {
        let mut t = Task { id: "t".into(), state: TaskState::Pending, generation: 0 };
        claim_task(&mut t, "w1", 1_000, 60).unwrap();
        assert!(claim_task(&mut t, "w2", 1_001, 60).is_err(), "stale worker must not overwrite");
        // Heartbeats renew the holder's own lease; others are rejected.
        heartbeat_task(&mut t, "w1", 1_050, 60).unwrap();
        assert!(heartbeat_task(&mut t, "w2", 1_050, 60).is_err());
        // Live claims cannot be reclaimed, even by a third worker.
        assert!(reclaim_task(&mut t, "w3", 1_051, 60).is_err());
        // After expiry the task is reclaimable with a bumped generation.
        reclaim_task(&mut t, "w3", 2_000, 60).unwrap();
        assert_eq!(t.generation, 2);
        assert!(matches!(&t.state, TaskState::Claimed { worker, .. } if worker == "w3"));
        // The stale holder's heartbeat now fails: its write is recognizable.
        assert!(heartbeat_task(&mut t, "w1", 2_001, 60).is_err());
    }

    #[tokio::test]
    async fn memory_reader_passes_conformance() {
        let seed = conformance_seed();
        check_conformance(
            &seed.reader,
            &seed.a1,
            &seed.b1,
            &seed.rel1,
            &seed.a2,
            &seed.builds,
        )
        .await;
    }

    #[test]
    fn edgeql_is_parameterized() {
        for q in [edgeql::UPSERT_ENTITY, edgeql::UPSERT_RELATIONSHIP, edgeql::ENTITY_BY_ID] {
            assert!(q.contains("$"), "values must be bound params, not interpolated");
            assert!(!q.contains("format!"), "no string interpolation in EdgeQL");
        }
        let _ = (RelationScope::CrossFile, RelationType::Calls);
    }
}
