//! In-memory [`GraphQueries`](crate::GraphQueries) fake used by conformance tests.

use std::collections::BTreeMap;

use chaosbox_core::GraphBuild;

use crate::StoreError;
use crate::queries::GraphQueries;
use crate::rows::{BuildRow, EntityRow, EvidenceRow, RelRow, entity_row, rel_row};

/// In-memory [`GraphQueries`] fake: same method surface and ordering as the
/// live path, so conformance tests prove parity. The `TypeDB` reader runs the
/// same suite against a live server (see `check_conformance`).
#[derive(Default)]
pub struct MemoryReader {
    builds: BTreeMap<String, GraphBuild>,
    active: BTreeMap<String, String>,
    evidence: BTreeMap<(String, String), Vec<EvidenceRow>>,
}

impl MemoryReader {
    /// An empty reader with no builds and no active pointers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the actual published store, including source-linked claims.
    #[must_use]
    pub fn from_store(store: &crate::MemoryStore) -> Self {
        Self {
            builds: store.builds.clone(),
            active: store.active.clone(),
            evidence: store.sealed.clone(),
        }
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
    pub fn attach_evidence(&mut self, rel_id: &str, rows: &[EvidenceRow]) {
        for build in self
            .builds
            .values()
            .filter(|b| b.edges.contains_key(rel_id))
        {
            self.evidence
                .insert((build.id.clone(), rel_id.to_owned()), rows.to_owned());
        }
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
impl GraphQueries for MemoryReader {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError> {
        match self.active.get(repo) {
            Some(id) => self.published_build(repo, id).await,
            None => Ok(None),
        }
    }

    async fn published_build(
        &self,
        repo: &str,
        build_id: &str,
    ) -> Result<Option<BuildRow>, StoreError> {
        Ok(self
            .builds
            .get(build_id)
            .filter(|b| b.repo == repo)
            .map(|b| {
                let mut snapshots = b.snapshot_ids.clone();
                snapshots.sort();
                BuildRow {
                    build_id: b.id.clone(),
                    generation: i64::try_from(b.generation).expect("generation fits in i64"),
                    status: "active".to_owned(),
                    snapshots,
                    coverage: b.coverage.clone(),
                    evidence_sealed: b
                        .edges
                        .keys()
                        .all(|rel| self.evidence.contains_key(&(b.id.clone(), rel.clone()))),
                }
            }))
    }

    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        let needle = unescape_like(like).to_lowercase();
        let limit = usize::try_from(limit.max(0)).expect("limit fits in usize");
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
    ) -> Result<Option<EntityRow>, StoreError> {
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
    ) -> Result<Vec<RelRow>, StoreError> {
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
    ) -> Result<Vec<RelRow>, StoreError> {
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
    ) -> Result<Vec<EntityRow>, StoreError> {
        let limit = usize::try_from(limit.max(0)).expect("limit fits in usize");
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
    ) -> Result<Vec<RelRow>, StoreError> {
        let limit = usize::try_from(limit.max(0)).expect("limit fits in usize");
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
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        self.evidence_for_limited(build_id, rel_id, i64::MAX).await
    }

    async fn evidence_for_limited(
        &self,
        build_id: &str,
        rel_id: &str,
        limit: i64,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        if self
            .builds
            .get(build_id)
            .is_none_or(|b| !b.edges.contains_key(rel_id))
        {
            return Ok(Vec::new());
        }
        let mut rows: Vec<_> = self
            .evidence
            .get(&(build_id.to_owned(), rel_id.to_owned()))
            .ok_or(StoreError::EvidenceClosureUnavailable)?
            .iter()
            .collect();
        rows.sort_by(|a, b| a.evidence_id.cmp(&b.evidence_id));
        Ok(rows
            .into_iter()
            .take(usize::try_from(limit.max(0)).unwrap_or(usize::MAX))
            .cloned()
            .collect())
    }
}
