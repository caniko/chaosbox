//! Read-only query surface shared by the live and in-memory readers.

use crate::StoreError;
use crate::rows::{BuildRow, EntityRow, EvidenceRow, RelRow};

/// Read-only query surface shared by the `TypeDB` reader and the
/// in-memory fake. Member reads are scoped to one explicit build id. Trusted
/// callers must resolve ownership through `active_build` or `published_build`
/// before using member queries; raw query methods are not an authorization API.
/// Ordering: entity lists come back ordered by qualified name; relationship
/// and evidence lists have unspecified order (conformance compares as sets).
/// An empty relation-type filter matches nothing (mirrors `array_unpack([])`).
#[async_trait::async_trait]
pub trait GraphQueries: Send + Sync {
    /// Active build header for a repository (per-request build pinning).
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError>;
    /// Published build owned by `repo`, including historical builds. Missing,
    /// foreign, and unpublished builds return `None` without exposing members.
    /// Legacy publications remain available for operator membership diffs;
    /// run-bound callers must also require the header's `evidence_sealed` flag.
    async fn published_build(
        &self,
        repo: &str,
        build_id: &str,
    ) -> Result<Option<BuildRow>, StoreError>;
    /// Bounded substring search over the pinned build's entity names
    /// (`like` carries `%` wrappers; `\` escapes `%` and `_`).
    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError>;
    /// Typed entity lookup within the pinned build.
    async fn entity_by_id(&self, build_id: &str, id: &str)
        -> Result<Option<EntityRow>, StoreError>;
    /// Outgoing relationships within the pinned build, with a type filter.
    async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError>;
    /// Incoming relationships within the pinned build, with a type filter.
    async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError>;
    /// Member entities of one build, bounded.
    async fn build_entities(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError>;
    /// Member relationships of one build, bounded.
    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, StoreError>;
    /// Bounded undirected neighborhood, ordered by neighboring entity id then
    /// relationship id. Keeps original direction; self edges appear once.
    /// Backends must apply the row limit before returning a projection.
    async fn adjacent_relationships(
        &self,
        _build_id: &str,
        _id: &str,
        _limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        Err(StoreError::Query(
            "bounded neighborhoods unavailable".into(),
        ))
    }
    /// Immutable bounded analytics computed at publication, when available.
    /// `None` identifies a legacy build; readers may use its bounded export.
    async fn navigation_summary(
        &self,
        _build_id: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(None)
    }
    /// Evidence attached to one relationship of the pinned build.
    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, StoreError>;
    /// Evidence bounded at the backend, ordered by evidence id. Callers fetch
    /// one extra row to distinguish a complete result from a budget failure.
    async fn evidence_for_limited(
        &self,
        build_id: &str,
        rel_id: &str,
        limit: i64,
    ) -> Result<Vec<EvidenceRow>, StoreError>;
}
