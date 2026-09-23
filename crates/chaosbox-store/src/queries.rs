//! Read-only query surface shared by the live and in-memory readers.

use crate::StoreError;
use crate::rows::{BuildRow, EntityRow, EvidenceRow, RelRow};

/// Read-only query surface shared by the `TypeDB` reader and the
/// in-memory fake. Every read is scoped to one pinned build id: readers can
/// never observe entities or relationships outside the active build.
/// Ordering: entity lists come back ordered by qualified name; relationship
/// and evidence lists have unspecified order (conformance compares as sets).
/// An empty relation-type filter matches nothing (mirrors `array_unpack([])`).
#[async_trait::async_trait]
pub trait GraphQueries: Send + Sync {
    /// Active build header for a repository (per-request build pinning).
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError>;
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
    /// Evidence attached to one relationship of the pinned build.
    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, StoreError>;
}
