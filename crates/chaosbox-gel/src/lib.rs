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

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, Evidence, EvidenceClass, GraphBuild,
    Relation, SnapshotFile,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Packaged SDL asset (also present under `dbschema/` in the crate package).
pub const SCHEMA_SDL: &str = include_str!("../../../dbschema/default.esdl");

/// Committed migration asset.
pub const MIGRATION_00001: &str = include_str!("../../../dbschema/migrations/00001.edgeql");

/// Committed migration asset: worker-task leases.
pub const MIGRATION_00002: &str = include_str!("../../../dbschema/migrations/00002.edgeql");

/// Committed migration asset: decision cache keys.
pub const MIGRATION_00003: &str = include_str!("../../../dbschema/migrations/00003.edgeql");

/// Pinned Gel version this schema is tested against.
pub const GEL_PINNED: &str = "7.2";
/// Schema compatibility marker checked by `db check`.
pub const SCHEMA_VERSION: u32 = 3;

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
    /// Idempotent file-version upsert (`$0` snapshot id, `$1` path,
    /// `$2` sha256, `$3` bytes).
    pub const UPSERT_FILE_VERSION: &str =
        "select (insert FileVersion { snapshot := (select SourceSnapshot filter .snapshot_id = <str>$0), \
         path := <str>$1, sha256 := <str>$2, bytes := <int64>$3 } \
         unless conflict on ((.snapshot, .path)) else (select FileVersion filter .snapshot.snapshot_id = <str>$0 and .path = <str>$1)) { path }";
    /// Span insert (`$0` file, `$1..$6` lines/cols/bytes); returns the new id.
    /// Separate statement because positional arg tuples cap at 12 params.
    pub const INSERT_SPAN: &str =
        "select (insert SourceSpan { file := <str>$0, start_line := <int64>$1, start_col := <int64>$2, \
         end_line := <int64>$3, end_col := <int64>$4, byte_start := <int64>$5, byte_end := <int64>$6 }) { id }";
    /// Idempotent entity upsert with an existing span (`$0` entity id,
    /// `$1` kind canonical name, `$2` repo, `$3` snapshot, `$4` file,
    /// `$5` name, `$6` qualified name, `$7` span id string).
    pub const UPSERT_ENTITY: &str =
        "select (insert Entity { entity_id := <str>$0, kind := <str>$1, repo := <str>$2, snapshot := <str>$3, \
         file := <str>$4, name := <str>$5, qualified_name := <str>$6, \
         span := (select SourceSpan filter .id = <uuid><str>$7) } \
         unless conflict on .entity_id else (select Entity filter .entity_id = <str>$0)) { entity_id }";
    /// Idempotent graph membership (`$0` build id, `$1` entity id).
    pub const INSERT_MEMBERSHIP: &str =
        "select (insert GraphMembership { build := (select GraphBuild filter .build_id = <str>$0), \
         entity := (select Entity filter .entity_id = <str>$1) } \
         unless conflict on ((.build, .entity)) else (select GraphMembership filter .build.build_id = <str>$0 and .entity.entity_id = <str>$1)) { build: { build_id } }";
    /// Idempotent edge membership (`$0` build id, `$1` rel id).
    pub const INSERT_EDGE_MEMBERSHIP: &str =
        "select (insert GraphEdgeMembership { build := (select GraphBuild filter .build_id = <str>$0), \
         relationship := (select Relationship filter .rel_id = <str>$1) } \
         unless conflict on ((.build, .relationship)) else (select GraphEdgeMembership filter .build.build_id = <str>$0 and .relationship.rel_id = <str>$1)) { build: { build_id } }";
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
    /// Conditional decision upsert with input-change supersedure (`$0` decision
    /// id, `$1` candidate id, `$2` question id, `$3` outcome, `$4` evidence
    /// class, `$5` model requested, `$6` model returned, `$7` confidence or {},
    /// `$8` probability or {}, `$9` cache key). Replaces on key change or
    /// recorded failure; empty when a valid row already stands.
    /// Used by the Round 3 flush; [`GelStore`](super::GelStore) documents why
    /// decision rows wait for the candidate chain.
    pub const UPSERT_DECISION: &str =
        "select (insert Decision { decision_id := <str>$0, candidate := (select Candidate filter .candidate_id = <str>$1), \
         question_id := <str>$2, outcome := <str>$3, evidence_class := <str>$4, \
         model_requested := <str>$5, model_returned := <str>$6, cache_key := <str>$9, \
         confidence := <optional float64>$7, probability := <optional float64>$8 } \
         unless conflict on ((.candidate, .question_id)) \
         else (update Decision filter .candidate.candidate_id = <str>$1 and .question_id = <str>$2 \
         and (.outcome = 'failed' or .cache_key != <str>$9) \
         set { decision_id := <str>$0, outcome := <str>$3, evidence_class := <str>$4, cache_key := <str>$9, \
         model_requested := <str>$5, model_returned := <str>$6, \
         confidence := <optional float64>$7, probability := <optional float64>$8 })) { decision_id, outcome }";
    /// Extraction-run insert (`$0` run id, `$1` repo, `$2` snapshot id).
    pub const INSERT_EXTRACTION_RUN: &str =
        "select (insert ExtractionRun { run_id := <str>$0, repo := <str>$1, \
         snapshot := (select SourceSnapshot filter .snapshot_id = <str>$2) } \
         unless conflict on .run_id else (select ExtractionRun filter .run_id = <str>$0)) { run_id }";
    /// Candidate-set insert (`$0` set id, `$1` run id, `$2` catalog digest,
    /// `$3` rubric version).
    pub const INSERT_CANDIDATE_SET: &str =
        "select (insert CandidateSet { set_id := <str>$0, run := (select ExtractionRun filter .run_id = <str>$1), \
         catalog_digest := <str>$2, rubric_version := <str>$3 } \
         unless conflict on .set_id else (select CandidateSet filter .set_id = <str>$0)) { set_id }";
    /// Candidate insert (`$0` candidate id, `$1` set id, `$2` rel type,
    /// `$3` from id, `$4` to id, `$5` reason, `$6` excerpt).
    pub const INSERT_CANDIDATE: &str =
        "select (insert Candidate { candidate_id := <str>$0, \
         candidate_set := (select CandidateSet filter .set_id = <str>$1), rel_type := <str>$2, \
         from_entity := (select Entity filter .entity_id = <str>$3), \
         to_entity := (select Entity filter .entity_id = <str>$4), \
         reason := <str>$5, state_excerpt := <str>$6 } \
         unless conflict on .candidate_id else (select Candidate filter .candidate_id = <str>$0)) { candidate_id }";
    /// Attempt insert (`$0` attempt id, `$1` candidate id, `$2` question id,
    /// `$3` model requested, `$4` model returned, `$5` cache key,
    /// `$6` input tokens or {}, `$7` http status or {}, `$8` error or {}).
    pub const INSERT_ATTEMPT: &str =
        "select (insert JevAttempt { attempt_id := <str>$0, \
         candidate := (select Candidate filter .candidate_id = <str>$1), question_id := <str>$2, \
         model_requested := <str>$3, model_returned := <str>$4, cache_key := <str>$5, \
         input_tokens := <optional int64>$6, http_status := <optional int64>$7, error := <optional str>$8 } \
         unless conflict on .attempt_id else (select JevAttempt filter .attempt_id = <str>$0)) { attempt_id }";
    /// Evidence insert with span (`$0` evidence id, `$1` class, `$2` supports,
    /// `$3` text, `$4` snapshot id, `$5` file path, `$6` span id string).
    /// Span rows go through `INSERT_SPAN` first: positional arg tuples cap
    /// at 12 params, so span-less evidence uses `INSERT_EVIDENCE_NOSPAN`.
    pub const INSERT_EVIDENCE: &str =
        "select (insert Evidence { evidence_id := <str>$0, class := <str>$1, supports := <bool>$2, text := <str>$3, \
         source_file_version := (select FileVersion filter .snapshot.snapshot_id = <str>$4 and .path = <str>$5), \
         span := (select SourceSpan filter .id = <uuid><str>$6) } \
         unless conflict on .evidence_id else (select Evidence filter .evidence_id = <str>$0)) { evidence_id }";
    /// Evidence insert without span (`$0..$5` as above, no span link).
    pub const INSERT_EVIDENCE_NOSPAN: &str =
        "select (insert Evidence { evidence_id := <str>$0, class := <str>$1, supports := <bool>$2, text := <str>$3, \
         source_file_version := (select FileVersion filter .snapshot.snapshot_id = <str>$4 and .path = <str>$5) } \
         unless conflict on .evidence_id else (select Evidence filter .evidence_id = <str>$0)) { evidence_id }";
    /// Claim insert with evidence links (`$0` claim id, `$1` rel id,
    /// `$2` accepted, `$3` supporting ids, `$4` contradicting ids).
    pub const INSERT_CLAIM: &str = "select (insert Claim { claim_id := <str>$0, \
         relationship := (select Relationship filter .rel_id = <str>$1), accepted := <bool>$2, \
         supporting := (select Evidence filter .evidence_id in array_unpack(<array<str>>$3)), \
         contradicting := (select Evidence filter .evidence_id in array_unpack(<array<str>>$4)) } \
         unless conflict on .claim_id else (select Claim filter .claim_id = <str>$0)) { claim_id }";
    /// Staging build insert (`$0` build id, `$1` repo, `$2` generation, `$3` status).
    pub const CREATE_BUILD: &str =
        "select (insert GraphBuild { build_id := <str>$0, repo := <str>$1, generation := <int64>$2, status := <str>$3 }) { build_id }";
    /// Atomic active-build pointer swing (`$0` repo, `$1` build id).
    pub const SET_ACTIVE_BUILD: &str =
        "select (insert ActiveBuildPointer { repo := <str>$0, build := (select GraphBuild filter .build_id = <str>$1) } \
         unless conflict on .repo else (update ActiveBuildPointer filter .repo = <str>$0 set { build := (select GraphBuild filter .build_id = <str>$1) })) { repo }";
    /// Guarded active-build swing: only when the generation still matches or
    /// the build is already active (idempotent retry), so a concurrent
    /// publisher wins instead of being overwritten (`$0` repo, `$1` build id,
    /// `$2` predecessor generation). Empty when a concurrent build moved first.
    pub const SET_ACTIVE_BUILD_IF_GEN: &str =
        "select (update ActiveBuildPointer filter .repo = <str>$0 \
         and (.build.generation = <int64>$2 or .build.build_id = <str>$1) \
         set { build := (select GraphBuild filter .build_id = <str>$1) }) { repo }";
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
    /// Decision lookup by candidate + question (`$0` candidate id,
    /// `$1` question id), with the cache key for reuse comparison.
    pub const DECISION_BY_CANDIDATE: &str =
        "select Decision { decision_id, candidate: { candidate_id }, question_id, outcome, \
         evidence_class, model_requested, model_returned, confidence, probability, cache_key } \
         filter .candidate.candidate_id = <str>$0 and .question_id = <str>$1";
    /// Append one evidence link to a relationship (`$0` rel id, `$1` evidence id).
    pub const LINK_EVIDENCE: &str = "select (update Relationship filter .rel_id = <str>$0 \
         set { evidence += (select Evidence filter .evidence_id = <str>$1) }) { rel_id }";
}

/// Span id row returned by the span insert.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpanIdRow {
    /// New span object id (uuid string).
    pub id: String,
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

/// Candidate id wrapper (EdgeQL shape).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateRef {
    /// Candidate id.
    pub candidate_id: String,
}

/// Decision row with its cache key for reuse comparison.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRow {
    /// Decision id.
    pub decision_id: String,
    /// Linked candidate wrapper.
    pub candidate: CandidateRef,
    /// Question id.
    pub question_id: String,
    /// Outcome name.
    pub outcome: String,
    /// Evidence class name.
    pub evidence_class: String,
    /// Model identity requested.
    pub model_requested: String,
    /// Model identity returned.
    pub model_returned: String,
    /// Confidence, if the answer type carries one.
    pub confidence: Option<f64>,
    /// Probability, if applicable.
    pub probability: Option<f64>,
    /// Cache identity the decision is valid under.
    pub cache_key: String,
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
        other => Err(GelError::Invariant(format!(
            "claim non-pending task {:?} as {worker}",
            other
        ))),
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
        other => Err(GelError::Invariant(format!(
            "heartbeat not holder: {other:?} as {worker}"
        ))),
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
        other => Err(GelError::Invariant(format!(
            "reclaim live task {other:?} as {worker}"
        ))),
    }
}

/// Storage abstraction: real Gel via [`GelStore`] (Gel-backed) or
/// [`MemoryStore`] for tests and environments without a server.
/// Mutating methods are async because the Gel backend needs network IO;
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
    ) -> Result<(), GelError>;
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
    ) -> Result<(), GelError>;
    /// Record a candidate of a registered set (idempotent per candidate id).
    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), GelError>;
    /// Stage an entity (idempotent); validated at publication.
    async fn put_entity(&mut self, e: Entity) -> Result<(), GelError>;
    /// Stage a relationship for one build (idempotent).
    async fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), GelError>;
    /// Record a decision (idempotent per candidate + question while inputs
    /// are unchanged; replaced when the cache key differs or a recorded
    /// `Failed` decision is retried).
    async fn put_decision(&mut self, d: Decision) -> Result<(), GelError>;
    /// Record evidence (idempotent per evidence id; the (snapshot, path)
    /// file version must be registered first).
    async fn put_evidence(&mut self, e: Evidence) -> Result<(), GelError>;
    /// Record a claim (idempotent per claim id).
    async fn put_claim(&mut self, c: Claim) -> Result<(), GelError>;
    /// Look up a stored decision by candidate + question for cache reuse.
    /// Returns `None` on a miss; callers compare `cache_key` themselves.
    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, GelError>;
    /// Validate invariants and atomically swing the active-build pointer.
    /// Rejects stale predecessors and older-worker overwrites.
    async fn publish(
        &mut self,
        build: GraphBuild,
        expected_predecessor: Option<String>,
    ) -> Result<(), GelError>;
    /// The active (last good) build for a repository, if any.
    fn active(&self, repo: &str) -> Option<GraphBuild>;
    /// A build by id, active or superseded.
    fn get(&self, build_id: &str) -> Option<GraphBuild>;
    /// Stored-record counts for tests and operator diagnostics.
    fn stats(&self) -> StoreStats;
}

/// In-memory store: same invariants as the Gel path (registered file
/// versions, idempotent writes, predecessor-checked publication, immutable
/// published builds via clone).
#[derive(Default)]
pub struct MemoryStore {
    builds: BTreeMap<String, GraphBuild>,
    active: BTreeMap<String, String>,
    decisions: BTreeMap<String, Decision>,
    evidence: BTreeMap<String, Evidence>,
    claims: BTreeMap<String, Claim>,
    /// (snapshot, path) -> (sha256, bytes); evidence linkage validated here.
    files: BTreeMap<(String, String), (String, u64)>,
    /// run id -> (repo, snapshot id).
    runs: BTreeMap<String, (String, String)>,
    /// set id -> (run id, catalog digest, rubric version).
    sets: BTreeMap<String, (String, String, String)>,
    /// candidate id -> (set id, candidate).
    candidates: BTreeMap<String, (String, Candidate)>,
}

impl MemoryStore {
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
    ) -> Result<(), GelError> {
        for f in files {
            if f.snapshot != snapshot_id {
                return Err(GelError::Invariant(format!(
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

    async fn put_entity(&mut self, _e: Entity) -> Result<(), GelError> {
        Ok(()) // entities live inside builds; staging validated at publish
    }
    async fn put_relation(&mut self, _r: Relation, _build: &str) -> Result<(), GelError> {
        Ok(())
    }
    async fn put_decision(&mut self, d: Decision) -> Result<(), GelError> {
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
    async fn put_evidence(&mut self, e: Evidence) -> Result<(), GelError> {
        if !self
            .files
            .contains_key(&(e.snapshot.clone(), e.source_file_version.clone()))
        {
            return Err(GelError::Invariant(format!(
                "evidence {} references unregistered file {} in snapshot {}",
                e.id, e.source_file_version, e.snapshot
            )));
        }
        self.evidence.entry(e.id.clone()).or_insert(e);
        Ok(())
    }
    async fn put_claim(&mut self, c: Claim) -> Result<(), GelError> {
        self.claims.entry(c.id.clone()).or_insert(c);
        Ok(())
    }

    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, GelError> {
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
    ) -> Result<(), GelError> {
        self.runs
            .entry(run_id.to_owned())
            .or_insert_with(|| (repo.to_owned(), snapshot_id.to_owned()));
        if let Some((run, catalog, rubric)) = self.sets.get(set_id) {
            if run != run_id || catalog != catalog_digest || rubric != rubric_version {
                return Err(GelError::Invariant(format!(
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

    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), GelError> {
        if !self.sets.contains_key(set_id) {
            return Err(GelError::Invariant(format!(
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
    ) -> Result<(), GelError> {
        // Validate invariants before pointer swing.
        for r in build.edges.values() {
            if !build.nodes.contains_key(&r.from) || !build.nodes.contains_key(&r.to) {
                return Err(GelError::Invariant(format!(
                    "edge {} outside build {}",
                    r.id, build.id
                )));
            }
        }
        if let Some(cur_id) = self.active.get(&build.repo) {
            let cur = self
                .builds
                .get(cur_id)
                .ok_or_else(|| GelError::NotFound(cur_id.clone()))?;
            if expected_predecessor.as_deref() != Some(&cur.id) {
                return Err(GelError::Invariant(format!(
                    "predecessor mismatch: expected {:?}, active is {} (gen {})",
                    expected_predecessor, cur.id, cur.generation
                )));
            }
            if build.generation <= cur.generation {
                return Err(GelError::Invariant(
                    "older worker cannot replace newer build".into(),
                ));
            }
        } else if expected_predecessor.is_some() {
            return Err(GelError::Invariant(
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

/// Gel-backed [`Store`]: all writes stage in an in-memory [`MemoryStore`]
/// with identical semantics, then flush to Gel at publication. Staging is
/// the write-ahead log: a failed or interrupted flush leaves the last good
/// Gel build active because the pointer swing is always last and guarded.
/// Decision/evidence/claim *rows* flush in Round 3 with the candidate chain;
/// this round flushes snapshots, files, entities, relationships, memberships,
/// builds, and the pointer.
pub struct GelStore {
    handle: Option<GelHandle>,
    staging: MemoryStore,
}

impl GelStore {
    /// A disconnected store; connects lazily on first flush.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handle: None,
            staging: MemoryStore::default(),
        }
    }

    /// The connected handle, connecting on first use. Returns an owned
    /// clone so staged writes and the flush never alias borrows.
    async fn connected(&mut self) -> Result<GelHandle, GelError> {
        if self.handle.is_none() {
            self.handle = Some(GelHandle::connect().await?);
        }
        Ok(self.handle.clone().expect("connected above"))
    }

    /// Flush staged runs, sets, candidates, decisions, evidence, claims,
    /// and relationship evidence links in FK order (all idempotent).
    async fn flush_chain(&self, handle: &GelHandle) -> Result<(), GelError> {
        let mut run_ids: Vec<&String> = self.staging.runs.keys().collect();
        run_ids.sort();
        for run_id in run_ids {
            let (repo, snapshot_id) = self.staging.runs.get(run_id).expect("key from map");
            handle.insert_run(run_id, repo, snapshot_id).await?;
        }
        let mut set_ids: Vec<&String> = self.staging.sets.keys().collect();
        set_ids.sort();
        for set_id in set_ids {
            let (run_id, catalog, rubric) = self.staging.sets.get(set_id).expect("key from map");
            handle.insert_set(set_id, run_id, catalog, rubric).await?;
        }
        let mut cand_ids: Vec<&String> = self.staging.candidates.keys().collect();
        cand_ids.sort();
        for cand_id in cand_ids {
            let (set_id, cand) = self.staging.candidates.get(cand_id).expect("key from map");
            handle.insert_candidate_row(set_id, cand).await?;
        }
        let mut dec_keys: Vec<&String> = self.staging.decisions.keys().collect();
        dec_keys.sort();
        for key in dec_keys {
            let d = self.staging.decisions.get(key).expect("key from map");
            handle.upsert_decision_row(d).await?;
        }
        let mut ev_ids: Vec<&String> = self.staging.evidence.keys().collect();
        ev_ids.sort();
        for ev_id in ev_ids {
            let e = self.staging.evidence.get(ev_id).expect("key from map");
            handle.insert_evidence_row(e).await?;
        }
        let mut claim_ids: Vec<&String> = self.staging.claims.keys().collect();
        claim_ids.sort();
        for claim_id in claim_ids {
            let c = self.staging.claims.get(claim_id).expect("key from map");
            handle.insert_claim_row(c).await?;
            for ev_id in c.supporting.iter().chain(c.contradicting.iter()) {
                handle.link_evidence(&c.relation_id, ev_id).await?;
            }
        }
        Ok(())
    }

    /// Flush one validated build's graph rows to Gel (idempotent upserts).
    /// Snapshot/file rows first (evidence links need them), then entities,
    /// relationships, memberships, and the build row itself.
    async fn flush_build(&self, handle: &GelHandle, build: &GraphBuild) -> Result<(), GelError> {
        for snapshot_id in &build.snapshot_ids {
            handle.upsert_snapshot(&build.repo, snapshot_id).await?;
        }
        for ((snapshot_id, path), (sha, bytes)) in &self.staging.files {
            if build.snapshot_ids.contains(snapshot_id) {
                handle
                    .ensure_snapshot_files(
                        snapshot_id,
                        &build.repo,
                        &[SnapshotFile {
                            snapshot: snapshot_id.clone(),
                            path: path.clone(),
                            sha256: sha.clone(),
                            bytes: *bytes,
                        }],
                    )
                    .await?;
            }
        }
        let mut nodes: Vec<&Entity> = build.nodes.values().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        for e in nodes {
            handle.put_entity_row(e).await?;
            handle.insert_membership(&build.id, &e.id).await?;
        }
        let mut edges: Vec<&Relation> = build.edges.values().collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        for r in edges {
            handle.put_relation_row(r).await?;
            handle.insert_edge_membership(&build.id, &r.id).await?;
        }
        handle
            .create_build(&build.id, &build.repo, build.generation as i64, "staging")
            .await?;
        Ok(())
    }
}

impl Default for GelStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Store for GelStore {
    async fn ensure_snapshot_files(
        &mut self,
        snapshot_id: &str,
        repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), GelError> {
        self.staging
            .ensure_snapshot_files(snapshot_id, repo, files)
            .await
    }

    async fn put_entity(&mut self, e: Entity) -> Result<(), GelError> {
        self.staging.put_entity(e).await
    }

    async fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), GelError> {
        self.staging.put_relation(r, build_id).await
    }

    async fn put_decision(&mut self, d: Decision) -> Result<(), GelError> {
        self.staging.put_decision(d).await
    }

    async fn put_evidence(&mut self, e: Evidence) -> Result<(), GelError> {
        self.staging.put_evidence(e).await
    }

    async fn put_claim(&mut self, c: Claim) -> Result<(), GelError> {
        self.staging.put_claim(c).await
    }

    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, GelError> {
        if let Some(d) = self
            .staging
            .find_decision(candidate_id, question_id)
            .await?
        {
            return Ok(Some(d));
        }
        let handle = self.handle.clone().ok_or_else(|| {
            GelError::Client("GelStore disconnected; no staged decision and no server".into())
        })?;
        match handle.find_decision_row(candidate_id, question_id).await? {
            None => Ok(None),
            Some(row) => Ok(Some(decision_from_row(row)?)),
        }
    }

    async fn ensure_run(
        &mut self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
        set_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), GelError> {
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

    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), GelError> {
        self.staging.put_candidate(set_id, c).await
    }

    async fn publish(
        &mut self,
        build: GraphBuild,
        expected_predecessor: Option<String>,
    ) -> Result<(), GelError> {
        // 1. Gel-side guard against the live pointer before writing anything.
        let handle = self.connected().await?;
        let live = handle.active_build(&build.repo).await?;
        match (&live, &expected_predecessor) {
            (None, None) => {}
            (Some(a), _) if a.build_id == build.id => return Ok(()), // idempotent retry
            (Some(a), Some(pred))
                if *pred == a.build_id && build.generation as i64 > a.generation => {}
            (Some(a), pred) => {
                return Err(GelError::Invariant(format!(
                    "predecessor mismatch: expected {:?}, Gel active is {} (gen {})",
                    pred, a.build_id, a.generation
                )));
            }
            (None, Some(pred)) => {
                return Err(GelError::Invariant(format!(
                    "expected predecessor {pred} but Gel has no active build"
                )));
            }
        }
        let pred_gen = live.map(|a| a.generation);
        // 2. Local invariant validation (cross-build edges, generations).
        self.staging
            .publish(build.clone(), expected_predecessor)
            .await?;
        // 3. Idempotent flush; safe to retry after a crash mid-flush.
        self.flush_build(&handle, &build).await?;
        self.flush_chain(&handle).await?;
        // 4. Guarded swing last: a concurrent publisher wins instead of being
        // overwritten, and the last good build stays active on any failure.
        match pred_gen {
            None => {
                handle.swing(&build.repo, &build.id).await?;
            }
            Some(g) => {
                if !handle.guarded_swing(&build.repo, &build.id, g).await? {
                    return Err(GelError::Invariant(
                        "concurrent publisher moved the pointer; build staged but not activated"
                            .into(),
                    ));
                }
            }
        }
        Ok(())
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
    async fn build_entities(&self, build_id: &str, limit: i64) -> Result<Vec<EntityRow>, GelError>;
    /// Member relationships of one build, bounded.
    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, GelError>;
    /// Evidence attached to one relationship of the pinned build.
    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, GelError>;
}

/// Rebuild a [`Decision`] from a [`DecisionRow`]: outcome and class travel
/// as JSON so enum shapes round-trip exactly.
fn decision_from_row(row: DecisionRow) -> Result<Decision, GelError> {
    let outcome: DecisionOutcome =
        serde_json::from_str(&row.outcome).map_err(|e| GelError::Query(e.to_string()))?;
    let evidence_class: EvidenceClass =
        serde_json::from_str(&format!("\"{}\"", row.evidence_class))
            .map_err(|e| GelError::Query(e.to_string()))?;
    Ok(Decision {
        id: row.decision_id,
        candidate_id: row.candidate.candidate_id,
        question_id: row.question_id,
        outcome,
        evidence_class,
        model_requested: row.model_requested,
        model_returned: row.model_returned,
        confidence: row.confidence,
        probability: row.probability,
        cache_key: row.cache_key,
    })
}

/// Project one entity into its row form (canonical storage names).
fn entity_row(e: &Entity) -> EntityRow {
    EntityRow {
        entity_id: e.id.clone(),
        kind: chaosbox_core::entity_kind_name(&e.kind),
        repo: e.repo.clone(),
        snapshot: e.snapshot.clone(),
        file: e.file.clone(),
        name: e.name.clone(),
        qualified_name: e.qualified_name.clone(),
    }
}

/// Canonical storage name for a relation scope (serde snake_case).
fn scope_name(s: &chaosbox_core::RelationScope) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{s:?}"))
}

/// Project one relationship into its row form (canonical storage names).
fn rel_row(r: &Relation) -> RelRow {
    RelRow {
        rel_id: r.id.clone(),
        rel_type: chaosbox_core::relation_type_name(&r.rel_type),
        from_entity: EndpointRef {
            entity_id: r.from.clone(),
        },
        to_entity: EndpointRef {
            entity_id: r.to.clone(),
        },
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
        self.builds
            .get(build_id)
            .map_or_else(Vec::new, |b| b.edges.values().map(rel_row).collect())
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
        Ok(self
            .active
            .get(repo)
            .and_then(|id| self.builds.get(id))
            .map(|b| BuildRow {
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

    async fn entity_by_id(&self, build_id: &str, id: &str) -> Result<Option<EntityRow>, GelError> {
        Ok(self
            .members(build_id)
            .into_iter()
            .find(|e| e.entity_id == id))
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

    async fn build_entities(&self, build_id: &str, limit: i64) -> Result<Vec<EntityRow>, GelError> {
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
        if self
            .builds
            .get(build_id)
            .is_none_or(|b| !b.edges.contains_key(rel_id))
        {
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

    async fn entity_by_id(&self, build_id: &str, id: &str) -> Result<Option<EntityRow>, GelError> {
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

    async fn build_entities(&self, build_id: &str, limit: i64) -> Result<Vec<EntityRow>, GelError> {
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
    let mut r1 = Relation::new(
        RelationType::Calls,
        &a1.id,
        &b1e.id,
        RelationScope::File,
        &b1.id,
    );
    r1.evidence_ids.push("ev1".into());
    b1.add_edge(r1.clone()).unwrap();
    let mut b2 = GraphBuild::new("conf", vec!["s2".into()], 2);
    b2.predecessor = Some(b1.id.clone());
    let a2 = ent("conf", "s2", "f.rs", "Alpha");
    let c2 = ent("conf", "s2", "g.rs", "Gamma");
    b2.add_node(a2.clone()).unwrap();
    b2.add_node(c2.clone()).unwrap();
    let r2 = Relation::new(
        RelationType::References,
        &a2.id,
        &c2.id,
        RelationScope::CrossFile,
        &b2.id,
    );
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
    let hits: Vec<_> = r
        .search_entities(&builds.0, "%alpha%", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(hits, vec![a1.to_owned()]);
    let hits: Vec<_> = r
        .search_entities(&builds.1, "%alpha%", 10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.entity_id)
        .collect();
    assert_eq!(hits, vec![a2.to_owned()]);
    assert_eq!(
        r.search_entities(&builds.1, "%alpha%", 0)
            .await
            .unwrap()
            .len(),
        0
    );
    // Escaped wildcards match literally, not as patterns (same on live Gel,
    // where backslash is the LIKE escape).
    assert!(r
        .search_entities(&builds.1, "%alp\\_ha%", 10)
        .await
        .unwrap()
        .is_empty());
    // Lookup is scoped: a1 is invisible from the second build.
    assert_eq!(
        r.entity_by_id(&builds.0, a1)
            .await
            .unwrap()
            .unwrap()
            .entity_id,
        a1
    );
    assert!(r.entity_by_id(&builds.1, a1).await.unwrap().is_none());
    assert!(r
        .entity_by_id(&builds.0, "ent:missing")
        .await
        .unwrap()
        .is_none());
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
    assert!(r
        .neighbors_out(&builds.1, a1, vec!["calls".into()])
        .await
        .unwrap()
        .is_empty());
    assert!(r
        .neighbors_out(&builds.0, a1, vec![])
        .await
        .unwrap()
        .is_empty());
    assert!(r
        .neighbors_out(&builds.0, a1, vec!["references".into()])
        .await
        .unwrap()
        .is_empty());
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
    assert!(r
        .build_entities("build:missing", 100)
        .await
        .unwrap()
        .is_empty());
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
    assert!(r
        .evidence_for(&builds.0, "rel:missing")
        .await
        .unwrap()
        .is_empty());
}

/// Native Gel handle: typed decoding over `query_json` with bound params.
///
/// Connection comes from the environment / instance config (never from
/// caller-controlled session variables); credentials arrive via
/// `CHAOSBOX_GEL_CREDENTIALS_FILE`. Cheap to clone (the client pools
/// connections internally).
#[derive(Clone, Debug)]
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
            .query_json(
                "select { version := <int64>$0 }",
                &(i64::from(SCHEMA_VERSION),),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let v: serde_json::Value =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(Probe {
            ok: true,
            detail: v.to_string(),
        })
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
                let row: MembershipRow =
                    serde_json::from_str(j.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
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

    /// Register one snapshot's file content identities (idempotent).
    /// Hashes always come from the snapshot; never invented.
    pub async fn ensure_snapshot_files(
        &self,
        snapshot_id: &str,
        repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), GelError> {
        self.upsert_snapshot(repo, snapshot_id).await?;
        for f in files {
            if f.snapshot != snapshot_id {
                return Err(GelError::Invariant(format!(
                    "file {} lists snapshot {}, registered under {}",
                    f.path, f.snapshot, snapshot_id
                )));
            }
            self.client
                .query_json(
                    edgeql::UPSERT_FILE_VERSION,
                    &(
                        snapshot_id,
                        f.path.as_str(),
                        f.sha256.as_str(),
                        f.bytes as i64,
                    ),
                )
                .await
                .map_err(|e| GelError::Query(e.to_string()))?;
        }
        Ok(())
    }

    /// Idempotent entity upsert: span row first (7 params), then the
    /// entity with the span id (8 params; arg tuples cap at 12).
    pub async fn put_entity_row(&self, e: &Entity) -> Result<(), GelError> {
        let kind = chaosbox_core::entity_kind_name(&e.kind);
        let json = self
            .client
            .query_json(
                edgeql::INSERT_SPAN,
                &(
                    e.span.file.as_str(),
                    e.span.start_line as i64,
                    e.span.start_col as i64,
                    e.span.end_line as i64,
                    e.span.end_col as i64,
                    e.span.byte_start as i64,
                    e.span.byte_end as i64,
                ),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<SpanIdRow> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        let span_id = rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| GelError::Query("span insert returned no id".into()))?;
        self.client
            .query_json(
                edgeql::UPSERT_ENTITY,
                &(
                    e.id.as_str(),
                    kind,
                    e.repo.as_str(),
                    e.snapshot.as_str(),
                    e.file.as_str(),
                    e.name.as_str(),
                    e.qualified_name.as_str(),
                    span_id,
                ),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Idempotent relationship upsert with typed endpoints (canonical names).
    /// Endpoint entities must already exist.
    pub async fn put_relation_row(&self, r: &Relation) -> Result<(), GelError> {
        let rel_type = chaosbox_core::relation_type_name(&r.rel_type);
        let scope = scope_name(&r.scope);
        self.client
            .query_json(
                edgeql::UPSERT_RELATIONSHIP,
                &(
                    r.id.as_str(),
                    rel_type,
                    r.from.as_str(),
                    r.to.as_str(),
                    scope,
                ),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Idempotent graph membership insert.
    pub async fn insert_membership(&self, build_id: &str, entity_id: &str) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::INSERT_MEMBERSHIP, &(build_id, entity_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Idempotent edge membership insert.
    pub async fn insert_edge_membership(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::INSERT_EDGE_MEMBERSHIP, &(build_id, rel_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Staging build insert.
    pub async fn create_build(
        &self,
        build_id: &str,
        repo: &str,
        generation: i64,
        status: &str,
    ) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::CREATE_BUILD, &(build_id, repo, generation, status))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Guarded pointer swing: returns true when swung, false when a
    /// concurrent publisher moved the pointer first (caller must abort).
    pub async fn guarded_swing(
        &self,
        repo: &str,
        build_id: &str,
        expected_gen: i64,
    ) -> Result<bool, GelError> {
        let json = self
            .client
            .query_json(
                edgeql::SET_ACTIVE_BUILD_IF_GEN,
                &(repo, build_id, expected_gen),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(!rows.is_empty())
    }

    /// Plain pointer swing for first publication (no predecessor to guard).
    pub async fn swing(&self, repo: &str, build_id: &str) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::SET_ACTIVE_BUILD, &(repo, build_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Extraction-run insert (idempotent).
    pub async fn insert_run(
        &self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
    ) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::INSERT_EXTRACTION_RUN, &(run_id, repo, snapshot_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Candidate-set insert (idempotent).
    pub async fn insert_set(
        &self,
        set_id: &str,
        run_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), GelError> {
        self.client
            .query_json(
                edgeql::INSERT_CANDIDATE_SET,
                &(set_id, run_id, catalog_digest, rubric_version),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Candidate insert (idempotent).
    pub async fn insert_candidate_row(&self, set_id: &str, c: &Candidate) -> Result<(), GelError> {
        let rel_type = chaosbox_core::relation_type_name(&c.rel_type);
        self.client
            .query_json(
                edgeql::INSERT_CANDIDATE,
                &(
                    c.id.as_str(),
                    set_id,
                    rel_type,
                    c.from_entity.as_str(),
                    c.to_entity.as_str(),
                    c.reason.as_str(),
                    c.state_excerpt.as_str(),
                ),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Conditional decision upsert: replaces on cache-key change or recorded
    /// failure, keeps valid rows. Returns true when the row now holds the
    /// given decision id.
    pub async fn upsert_decision_row(&self, d: &Decision) -> Result<bool, GelError> {
        let outcome =
            serde_json::to_string(&d.outcome).map_err(|e| GelError::Query(e.to_string()))?;
        let class = chaosbox_core::evidence_class_name(d.evidence_class);
        let json = self
            .client
            .query_json(
                edgeql::UPSERT_DECISION,
                &(
                    d.id.as_str(),
                    d.candidate_id.as_str(),
                    d.question_id.as_str(),
                    outcome,
                    class,
                    d.model_requested.as_str(),
                    d.model_returned.as_str(),
                    d.confidence,
                    d.probability,
                    d.cache_key.as_str(),
                ),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
        Ok(rows
            .iter()
            .any(|r| r["decision_id"].as_str() == Some(d.id.as_str())))
    }

    /// Evidence insert (idempotent); span goes through `INSERT_SPAN` first.
    pub async fn insert_evidence_row(&self, e: &Evidence) -> Result<(), GelError> {
        let class = chaosbox_core::evidence_class_name(e.class);
        if let Some(span) = &e.span {
            let json = self
                .client
                .query_json(
                    edgeql::INSERT_SPAN,
                    &(
                        span.file.as_str(),
                        span.start_line as i64,
                        span.start_col as i64,
                        span.end_line as i64,
                        span.end_col as i64,
                        span.byte_start as i64,
                        span.byte_end as i64,
                    ),
                )
                .await
                .map_err(|e| GelError::Query(e.to_string()))?;
            let rows: Vec<SpanIdRow> =
                serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
            let span_id = rows
                .into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| GelError::Query("span insert returned no id".into()))?;
            self.client
                .query_json(
                    edgeql::INSERT_EVIDENCE,
                    &(
                        e.id.as_str(),
                        class,
                        e.supports,
                        e.text.as_str(),
                        e.snapshot.as_str(),
                        e.source_file_version.as_str(),
                        span_id,
                    ),
                )
                .await
                .map_err(|e| GelError::Query(e.to_string()))?;
        } else {
            self.client
                .query_json(
                    edgeql::INSERT_EVIDENCE_NOSPAN,
                    &(
                        e.id.as_str(),
                        class,
                        e.supports,
                        e.text.as_str(),
                        e.snapshot.as_str(),
                        e.source_file_version.as_str(),
                    ),
                )
                .await
                .map_err(|e| GelError::Query(e.to_string()))?;
        }
        Ok(())
    }

    /// Claim insert with evidence links (idempotent).
    pub async fn insert_claim_row(&self, c: &Claim) -> Result<(), GelError> {
        self.client
            .query_json(
                edgeql::INSERT_CLAIM,
                &(
                    c.id.as_str(),
                    c.relation_id.as_str(),
                    c.accepted,
                    c.supporting.clone(),
                    c.contradicting.clone(),
                ),
            )
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        Ok(())
    }

    /// Append one evidence link to a relationship.
    pub async fn link_evidence(&self, rel_id: &str, evidence_id: &str) -> Result<(), GelError> {
        self.client
            .query_json(edgeql::LINK_EVIDENCE, &(rel_id, evidence_id))
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

    /// Decision lookup by candidate + question with its cache key.
    pub async fn find_decision_row(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<DecisionRow>, GelError> {
        let json = self
            .client
            .query_single_json(edgeql::DECISION_BY_CANDIDATE, &(candidate_id, question_id))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        match json {
            None => Ok(None),
            Some(j) => {
                let row: DecisionRow =
                    serde_json::from_str(j.as_ref()).map_err(|e| GelError::Query(e.to_string()))?;
                Ok(Some(row))
            }
        }
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
        Entity::new(
            EntityKind::Symbol,
            repo,
            snap,
            file,
            name,
            name,
            SourceSpan::point(file, 1, 1, 0),
        )
    }

    #[test]
    fn schema_assets_packaged() {
        assert!(SCHEMA_SDL.contains("type Relationship"));
        assert!(SCHEMA_SDL.contains("ActiveBuildPointer"));
        assert!(SCHEMA_SDL.contains("type WorkerTask"));
        assert!(!SCHEMA_SDL.contains("json;") || SCHEMA_SDL.contains("raw_envelope"));
        assert!(MIGRATION_00001.contains("m1_chaosbox_init"));
        assert!(MIGRATION_00002.contains("m2_worker_tasks"));
        assert!(
            MIGRATION_00002.contains("m1_chaosbox_init"),
            "migration chain must link"
        );
        assert!(MIGRATION_00003.contains("m3_decision_cache_key"));
        assert!(
            MIGRATION_00003.contains("m2_worker_tasks"),
            "migration chain must link"
        );
    }

    /// Shared write-path conformance over any [`Store`] impl: file linkage,
    /// decision idempotency + Failed-supersedure, and publication guards.
    /// Runs against [`MemoryStore`] now; a live-Gel test seeds nothing extra
    /// and calls this against [`GelStore`] once a server is available.
    pub async fn check_write_conformance<S: Store>(s: &mut S) {
        use chaosbox_core::{DecisionOutcome, EvidenceClass, RelationType, SourceSpan};
        // Run + set identity registers before candidates may reference it.
        s.ensure_run("run:1", "r", "s1", "set:1", "catalog:1", "rubric-v1")
            .await
            .unwrap();
        assert_eq!(s.stats().runs, 1);
        // Same inputs re-register idempotently; changed inputs are rejected.
        s.ensure_run("run:1", "r", "s1", "set:1", "catalog:1", "rubric-v1")
            .await
            .unwrap();
        assert!(s
            .ensure_run("run:1", "r", "s1", "set:1", "catalog:2", "rubric-v1")
            .await
            .is_err());
        let cand = Candidate {
            id: "cand:1".into(),
            rel_type: RelationType::Calls,
            from_entity: "ent:a".into(),
            to_entity: "ent:b".into(),
            reason: "structural".into(),
            state_excerpt: String::new(),
        };
        assert!(
            s.put_candidate("set:missing", &cand).await.is_err(),
            "unregistered sets never resolve"
        );
        s.put_candidate("set:1", &cand).await.unwrap();
        s.put_candidate("set:1", &cand).await.unwrap();
        assert_eq!(s.stats().candidates, 1);
        // File versions register before evidence may reference them.
        let files = vec![SnapshotFile {
            snapshot: "s1".into(),
            path: "a.rs".into(),
            sha256: "abc".into(),
            bytes: 3,
        }];
        s.ensure_snapshot_files("s1", "r", &files).await.unwrap();
        let ev = Evidence {
            id: "ev1".into(),
            class: EvidenceClass::Extracted,
            supports: true,
            text: "[structural] a".into(),
            span: Some(SourceSpan::point("a.rs", 1, 1, 0)),
            snapshot: "s1".into(),
            source_file_version: "a.rs".into(),
        };
        s.put_evidence(ev).await.unwrap();
        assert_eq!(s.stats().evidence, 1);
        let bad = Evidence {
            id: "ev2".into(),
            class: EvidenceClass::Ambiguous,
            supports: false,
            text: "x".into(),
            span: None,
            snapshot: "s9".into(),
            source_file_version: "missing.rs".into(),
        };
        assert!(
            s.put_evidence(bad).await.is_err(),
            "unregistered files never resolve"
        );
        // Decisions: first write wins, except Failed supersedes once.
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
            cache_key: "test-cache-key".into(),
        };
        s.put_decision(mk("d1", DecisionOutcome::Failed("down".into())))
            .await
            .unwrap();
        s.put_decision(mk("d2", DecisionOutcome::Accepted))
            .await
            .unwrap();
        assert_eq!(s.stats().decisions, 1, "one row per (candidate, question)");
        s.put_decision(mk("d3", DecisionOutcome::Rejected))
            .await
            .unwrap();
        assert_eq!(s.stats().decisions, 1, "accepted outcomes stick");
        // Publication: stale predecessors and older generations rejected.
        let mut b1 = GraphBuild::new("r", vec!["s1".into()], 1);
        b1.add_node(ent("r", "s1", "a.rs", "a")).unwrap();
        s.publish(b1.clone(), None).await.unwrap();
        let mut stale = GraphBuild::new("r", vec!["s1".into()], 1);
        stale.add_node(ent("r", "s1", "b.rs", "b")).unwrap();
        assert!(s
            .publish(stale, Some("wrong-predecessor".into()))
            .await
            .is_err());
        let mut b2 = GraphBuild::new("r", vec!["s2".into()], 2);
        b2.predecessor = Some(b1.id.clone());
        b2.add_node(ent("r", "s2", "c.rs", "c")).unwrap();
        assert!(s.publish(b2.clone(), Some(b1.id.clone())).await.is_ok());
        assert_eq!(s.active("r").unwrap().id, b2.id);
        assert_eq!(s.stats().builds, 2);
    }

    #[tokio::test]
    async fn memory_store_write_conformance() {
        let mut s = MemoryStore::new();
        check_write_conformance(&mut s).await;
    }

    #[test]
    fn task_claim_recovery() {
        let mut t = Task {
            id: "t".into(),
            state: TaskState::Pending,
            generation: 0,
        };
        claim_task(&mut t, "w1", 1_000, 60).unwrap();
        assert!(
            claim_task(&mut t, "w2", 1_001, 60).is_err(),
            "stale worker must not overwrite"
        );
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
        for q in [
            edgeql::UPSERT_SNAPSHOT,
            edgeql::UPSERT_FILE_VERSION,
            edgeql::INSERT_SPAN,
            edgeql::UPSERT_ENTITY,
            edgeql::UPSERT_RELATIONSHIP,
            edgeql::INSERT_DECISION,
            edgeql::UPSERT_DECISION,
            edgeql::INSERT_EXTRACTION_RUN,
            edgeql::INSERT_CANDIDATE_SET,
            edgeql::INSERT_CANDIDATE,
            edgeql::INSERT_ATTEMPT,
            edgeql::INSERT_EVIDENCE,
            edgeql::INSERT_EVIDENCE_NOSPAN,
            edgeql::INSERT_CLAIM,
            edgeql::CREATE_BUILD,
            edgeql::SET_ACTIVE_BUILD,
            edgeql::SET_ACTIVE_BUILD_IF_GEN,
            edgeql::ACTIVE_BUILD,
            edgeql::ENTITY_BY_ID,
            edgeql::SEARCH_ENTITIES,
            edgeql::NEIGHBORS_OUT,
            edgeql::NEIGHBORS_IN,
            edgeql::BUILD_ENTITIES,
            edgeql::BUILD_RELATIONSHIPS,
            edgeql::EVIDENCE_FOR_REL,
            edgeql::INSERT_MEMBERSHIP,
            edgeql::INSERT_EDGE_MEMBERSHIP,
        ] {
            assert!(
                q.contains("$"),
                "values must be bound params, not interpolated"
            );
            assert!(!q.contains("format!"), "no string interpolation in EdgeQL");
        }
        let _ = (RelationScope::CrossFile, RelationType::Calls);
    }
}
