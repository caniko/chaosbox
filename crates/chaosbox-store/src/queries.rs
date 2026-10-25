//! Read-only query surface shared by the live and in-memory readers.

use crate::StoreError;
use crate::rows::{BuildRow, EntityRow, EvidenceRow, RelRow};

/// Which endpoint of a relationship is anchored by a neighborhood query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeighborhoodDirection {
    /// The anchor is the source endpoint.
    Outgoing,
    /// The anchor is the target endpoint.
    Incoming,
}

/// Read-only query surface shared by the PostgreSQL reader and the
/// in-memory fake. Member reads are scoped to one explicit build id. Trusted
/// callers must resolve ownership through `active_build` or `published_build`
/// before using member queries; raw query methods are not an authorization API.
/// Ordering: entity lists come back ordered by qualified name; relationship
/// and evidence lists have unspecified order unless the method documents one.
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
    /// (`like` carries `%` wrappers; `\` escapes `%` and `_`), ranked exact
    /// name, then name prefix, then substring, by qualified name within each.
    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError>;
    /// Typed entity lookup within the pinned build.
    async fn entity_by_id(&self, build_id: &str, id: &str)
        -> Result<Option<EntityRow>, StoreError>;
    /// Exact outgoing/incoming relationship counts in one pinned build.
    /// Counts use the same type filter as neighborhoods, including self edges
    /// once in each direction. Backends must aggregate without returning edge
    /// collections; unsupported backends fail rather than fall back to fetching.
    async fn neighbor_counts(
        &self,
        _build_id: &str,
        _id: &str,
        _rel_types: Vec<String>,
    ) -> Result<(u64, u64), StoreError> {
        Err(StoreError::Query("relationship counts unavailable".into()))
    }
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
    /// One directional page ordered by relationship id, strictly after `after`.
    /// Build, anchor, direction, type filter and exclusive boundary are applied
    /// before the backend row limit. Empty filters and nonpositive limits return
    /// no rows. Self-loops appear once in each direction; repeated filter values
    /// do not duplicate rows. The caller may request page size + 1 for a sentinel.
    /// Unsupported backends fail rather than materialize an unbounded collection.
    /// This raw key is not an authorized continuation token: trusted callers must
    /// pin ownership and bind the complete request scope before resuming a page.
    async fn neighbor_page(
        &self,
        _build_id: &str,
        _id: &str,
        _direction: NeighborhoodDirection,
        _rel_types: Vec<String>,
        _after: Option<&str>,
        _limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        Err(StoreError::Query("neighborhood pages unavailable".into()))
    }
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
    /// Entities of the pinned build whose embedded text is nearest to a
    /// unit-length `query` vector under `model`, closest first. Entities
    /// without an embedding are absent; backends without vectors fail.
    async fn nearest_entities(
        &self,
        _build_id: &str,
        _model: &str,
        _query: &[f32],
        _limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        Err(StoreError::Query("semantic search unavailable".into()))
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
