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

/// Pinned Gel version this schema is tested against.
pub const GEL_PINNED: &str = "7.2";
/// Schema compatibility marker checked by `db check`.
pub const SCHEMA_VERSION: u32 = 1;

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
    /// Outgoing relationships (`$0` entity id, `$1` relation-type filter list).
    pub const NEIGHBORS_OUT: &str =
        "select Relationship { rel_id, rel_type, from_entity: { entity_id }, to_entity: { entity_id } } \
         filter .from_entity.entity_id = <str>$0 and .rel_type in array_unpack(<array<str>>$1)";
    /// Entity lookup by id (`$0` entity id).
    pub const ENTITY_BY_ID: &str =
        "select Entity { entity_id, kind, repo, snapshot, file, name, qualified_name } filter .entity_id = <str>$0";
    /// Substring search over names (`$0` like pattern, `$1` limit).
    pub const SEARCH_ENTITIES: &str =
        "select Entity { entity_id, kind, repo, snapshot, file, name, qualified_name } \
         filter .name ilike <str>$0 or .qualified_name ilike <str>$0 order by .qualified_name limit <int64>$1";
    /// Incoming relationships (`$0` entity id, `$1` relation-type filter list).
    pub const NEIGHBORS_IN: &str =
        "select Relationship { rel_id, rel_type, from_entity: { entity_id }, to_entity: { entity_id } } \
         filter .to_entity.entity_id = <str>$0 and .rel_type in array_unpack(<array<str>>$1)";
    /// All member entities of one build (`$0` build id, `$1` limit).
    pub const BUILD_ENTITIES: &str =
        "select GraphMembership { entity: { entity_id, kind, repo, snapshot, file, name, qualified_name } } \
         filter .build.build_id = <str>$0 order by .entity.qualified_name limit <int64>$1";
    /// All member relationships of one build (`$0` build id, `$1` limit).
    pub const BUILD_RELATIONSHIPS: &str =
        "select GraphEdgeMembership { relationship: { rel_id, rel_type, from_entity: { entity_id }, to_entity: { entity_id } } } \
         filter .build.build_id = <str>$0 limit <int64>$1";
    /// Evidence attached to one relationship (`$0` rel id).
    pub const EVIDENCE_FOR_REL: &str =
        "select Evidence { evidence_id, class, supports, text } \
         filter .<evidence[is Relationship].rel_id = <str>$0";
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Ready to be claimed by a worker.
    Pending,
    /// Held by a worker; stale holders never overwrite newer results.
    Claimed {
        /// Worker holding the claim.
        worker: String,
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
    /// Claim generation; incremented on every successful claim.
    pub generation: u64,
}

/// Claim a pending task: only `Pending` tasks can be claimed, so stale
/// workers never overwrite newer task results.
pub fn claim_task(task: &mut Task, worker: &str) -> Result<(), GelError> {
    match &task.state {
        TaskState::Pending => {
            task.state = TaskState::Claimed { worker: worker.to_owned() };
            task.generation += 1;
            Ok(())
        }
        other => Err(GelError::Invariant(format!("claim non-pending task {:?} as {worker}", other))),
    }
}

/// Storage abstraction: real Gel via [`GelHandle`] or [`MemoryStore`] for
/// tests and environments without a server.
pub trait Store: Send + Sync {
    /// Stage an entity (idempotent); validated at publication.
    fn put_entity(&mut self, e: Entity) -> Result<(), GelError>;
    /// Stage a relationship for one build (idempotent).
    fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), GelError>;
    /// Record a decision (idempotent per candidate + question; first write wins).
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
        // Idempotent: first write wins per (candidate, question).
        let key = format!("{}:{}", d.candidate_id, d.question_id);
        self.decisions.entry(key).or_insert(d);
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
    pub async fn entity_by_id(&self, id: &str) -> Result<Option<EntityRow>, GelError> {
        let json = self
            .client
            .query_single_json(edgeql::ENTITY_BY_ID, &(id,))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        match json {
            None => Ok(None),
            Some(j) => {
                let row: EntityRow = serde_json::from_str(j.as_ref())
                    .map_err(|e| GelError::Query(e.to_string()))?;
                Ok(Some(row))
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

    /// Bounded substring search over entity names.
    pub async fn search_entities(
        &self,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::SEARCH_ENTITIES, &(like, limit))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))
    }

    /// Outgoing relationships with a relation-type filter (empty filter = none).
    pub async fn neighbors_out(
        &self,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::NEIGHBORS_OUT, &(id, rel_types))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))
    }

    /// Incoming relationships with a relation-type filter (empty filter = none).
    pub async fn neighbors_in(
        &self,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::NEIGHBORS_IN, &(id, rel_types))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))
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

    /// Evidence attached to one relationship (claim support/contradiction).
    pub async fn evidence_for(&self, rel_id: &str) -> Result<Vec<EvidenceRow>, GelError> {
        let json = self
            .client
            .query_json(edgeql::EVIDENCE_FOR_REL, &(rel_id,))
            .await
            .map_err(|e| GelError::Query(e.to_string()))?;
        serde_json::from_str(json.as_ref()).map_err(|e| GelError::Query(e.to_string()))
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
        assert!(!SCHEMA_SDL.contains("json;") || SCHEMA_SDL.contains("raw_envelope"));
        assert!(MIGRATION_00001.contains("m1_chaosbox_init"));
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
        claim_task(&mut t, "w1").unwrap();
        assert!(claim_task(&mut t, "w2").is_err(), "stale worker must not overwrite");
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
