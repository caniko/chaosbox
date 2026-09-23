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
impl GraphQueries for MemoryReader {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError> {
        Ok(self
            .active
            .get(repo)
            .and_then(|id| self.builds.get(id))
            .map(|b| {
                let mut snapshots = b.snapshot_ids.clone();
                snapshots.sort();
                BuildRow {
                    build_id: b.id.clone(),
                    generation: i64::try_from(b.generation).expect("generation fits in i64"),
                    status: "active".to_owned(),
                    snapshots,
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
