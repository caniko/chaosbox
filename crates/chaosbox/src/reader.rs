//! Read-only consumer queries: LIKE escaping, relation filters, and the pinned-graph reader.

use super::PipelineError;

#[path = "reader_navigation.rs"]
mod navigation;
#[path = "reader_pages.rs"]
mod pages;
#[path = "reader_path.rs"]
mod path;

pub use pages::{NeighborhoodMode, NeighborhoodPage, NeighborhoodQuery, PreparedNeighborhood};
pub use path::{PathBudget, PathQuery, PreparedPath};

// ---- Read-only consumer path (CLI and MCP share this) ----

/// Escape LIKE wildcards (`\`, `%`, `_`) so user input matches literally.
/// Shared by both backends: the live path relies on backslash LIKE escapes.
fn escape_like(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for c in query.chars() {
        if c == '\\' || c == '%' || c == '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Relation vocabulary for relationship-type filters; an empty filter matches nothing, so
/// callers pass [`all_relation_types`] for unfiltered neighborhoods.
#[must_use]
pub fn all_relation_types() -> Vec<String> {
    [
        "contains",
        "defines",
        "imports",
        "references",
        "calls",
        "implements",
        "links_to",
        "mentions",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Validate a relation-type filter against the vocabulary (case-insensitive),
/// returning canonical names. Unknown names error loudly with the valid list
/// instead of silently matching nothing (empty means "no relations").
pub fn validate_rel_filter(
    filter: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, PipelineError> {
    match filter {
        None => Ok(None),
        Some(names) => {
            let vocab = all_relation_types();
            let mut out = Vec::with_capacity(names.len());
            for n in names {
                match vocab.iter().find(|v| v.eq_ignore_ascii_case(&n)) {
                    Some(canonical) => out.push(canonical.clone()),
                    None => {
                        return Err(PipelineError::Consumer(format!(
                            "unknown relation type {n:?}; valid: {}",
                            vocab.join(", ")
                        )));
                    }
                }
            }
            Ok(Some(out))
        }
    }
}

/// Hard cap for export projections; truncation is reported, never silent.
pub const EXPORT_NODE_CAP: i64 = 10_000;
/// Hard cap for exported edges.
pub const EXPORT_EDGE_CAP: i64 = 20_000;

/// Validate an exact-publication selector before credentials/backend acquisition.
/// Labels are opaque: validation never trims or rewrites historical identities.
pub fn validate_build_id(build_id: &str) -> Result<(), PipelineError> {
    if build_id.trim().is_empty() || build_id.len() > 256 || build_id.chars().any(char::is_control)
    {
        return Err(PipelineError::Consumer(
            "invalid build_id: expected nonblank 1..256 bytes without controls".into(),
        ));
    }
    Ok(())
}

/// Backend read-only queries. One build id is pinned per reader from the
/// active-build pointer or an exact published build; readers never mutate,
/// migrate, or load Jev credentials.
/// Generic over [`chaosbox_store::GraphQueries`] with `PgReader` as the
/// live backend and [`chaosbox_store::MemoryReader`] for tests.
pub struct GraphReader<R> {
    handle: R,
    repo: String,
    /// Pinned publication id for every request this reader serves.
    pub build_id: String,
    /// Pinned generation (predecessor/generation checks on the read side).
    pub generation: i64,
    /// Published build status (`active`, including historical publications).
    pub status: String,
    /// Snapshot ids pinned by the build: the freshness fingerprint status
    /// reports so consumers can detect a build that no longer matches its
    /// sources. Empty when the backend's projection predates the field.
    pub snapshots: Vec<String>,
    /// Processing coverage pinned with the build; unknown for legacy builds.
    pub coverage: Option<chaosbox_core::coverage::BuildCoverage>,
    /// Query embedder for semantic seeds; lexical-only when absent.
    embedder: Option<crate::embed::Embedder>,
}

/// Reciprocal rank fusion (k = 60) of ranked lists, best first. Ties keep
/// the earlier list's order, so a single list passes through unchanged.
pub(crate) fn rrf<T>(lists: Vec<Vec<T>>, key: impl Fn(&T) -> &str, limit: usize) -> Vec<T> {
    let mut fused: std::collections::HashMap<String, (f64, usize, T)> =
        std::collections::HashMap::new();
    let mut seen = 0;
    for list in lists {
        for (rank, item) in list.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let score = 1.0 / (61.0 + rank as f64);
            let slot = fused.entry(key(&item).to_owned()).or_insert_with(|| {
                seen += 1;
                (0.0, seen, item)
            });
            slot.0 += score;
        }
    }
    let mut ranked: Vec<_> = fused.into_values().collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, item)| item)
        .collect()
}

impl<R: chaosbox_store::GraphQueries> GraphReader<R> {
    /// Select current or one exact repository-owned publication without fallback.
    pub async fn selected(
        handle: R,
        repo: &str,
        build_id: Option<&str>,
    ) -> Result<Self, PipelineError> {
        match build_id {
            Some(build_id) => Self::published(handle, repo, build_id).await,
            None => Self::pinned(handle, repo).await,
        }
    }

    /// Pin the active build for `repo` on an existing query backend.
    /// Used by tests with [`chaosbox_store::MemoryReader`].
    pub async fn pinned(handle: R, repo: &str) -> Result<Self, PipelineError> {
        let build = handle
            .active_build(repo)
            .await
            .map_err(|e| PipelineError::Consumer(format!("active build: {e}")))?
            .ok_or_else(|| crate::no_active_build(repo))?;
        Ok(Self::from_header(handle, repo, build))
    }

    /// Pin one exact publication owned by `repo`, without consulting its active
    /// pointer. This repository-level reader does not grant scoped-view access.
    pub async fn published(handle: R, repo: &str, build_id: &str) -> Result<Self, PipelineError> {
        validate_build_id(build_id)?;
        let build = handle
            .published_build(repo, build_id)
            .await
            .map_err(|e| PipelineError::Consumer(format!("published build: {e}")))?
            .filter(|build| build.build_id == build_id)
            .ok_or_else(|| {
                PipelineError::Consumer("published build unavailable for repo".into())
            })?;
        Ok(Self::from_header(handle, repo, build))
    }

    fn from_header(handle: R, repo: &str, build: chaosbox_store::BuildRow) -> Self {
        Self {
            handle,
            repo: repo.to_owned(),
            build_id: build.build_id,
            generation: build.generation,
            status: build.status,
            snapshots: build.snapshots,
            coverage: build.coverage,
            embedder: None,
        }
    }

    /// Enable semantic seeds for `search` and `context`.
    #[must_use]
    pub fn with_embedder(mut self, embedder: Option<crate::embed::Embedder>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Nearest embedded entities of the pinned build, or `None` when
    /// semantic retrieval is unavailable (no embedder, endpoint down or
    /// slow, backend without vectors): callers then stay lexical.
    async fn semantic(&self, query: &str, limit: usize) -> Option<Vec<chaosbox_store::EntityRow>> {
        let embedder = self.embedder.as_ref().filter(|_| limit > 0)?;
        let vector = embedder.query(query).await.ok()?;
        self.handle
            .nearest_entities(
                &self.build_id,
                &embedder.key(),
                &vector,
                i64::try_from(limit).unwrap_or(i64::MAX),
            )
            .await
            .ok()
    }

    /// Bounded search over the pinned build: literal substring matches
    /// (LIKE wildcards escaped, exact and prefix names first) fused with the
    /// nearest embedded entities when semantic retrieval is available.
    pub async fn search(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<chaosbox_store::EntityRow>, PipelineError> {
        let like = format!("%{}%", escape_like(query));
        let lexical = self
            .handle
            .search_entities(&self.build_id, &like, limit)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        let n = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
        let mut lists = vec![lexical];
        lists.extend(self.semantic(query, n).await);
        Ok(rrf(lists, |e| e.entity_id.as_str(), n))
    }

    /// Typed entity lookup within the pinned build.
    pub async fn lookup(
        &self,
        id: &str,
    ) -> Result<Option<chaosbox_store::EntityRow>, PipelineError> {
        self.handle
            .entity_by_id(&self.build_id, id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))
    }

    /// Incoming/outgoing neighborhoods within the pinned build, with an
    /// optional relation filter. `None` means all relation types.
    pub async fn neighbors(
        &self,
        id: &str,
        filter: Option<Vec<String>>,
    ) -> Result<(Vec<chaosbox_store::RelRow>, Vec<chaosbox_store::RelRow>), PipelineError> {
        let types = validate_rel_filter(filter)?.unwrap_or_else(all_relation_types);
        let out = self
            .handle
            .neighbors_out(&self.build_id, id, types.clone())
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        let inc = self
            .handle
            .neighbors_in(&self.build_id, id, types)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        Ok((out, inc))
    }

    /// Deterministic export of the pinned build; truncation errors honestly.
    pub async fn export(&self) -> Result<serde_json::Value, PipelineError> {
        let entities = self
            .handle
            .build_entities(&self.build_id, EXPORT_NODE_CAP + 1)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        if i64::try_from(entities.len()).expect("entity count fits in i64") > EXPORT_NODE_CAP {
            return Err(PipelineError::Consumer(format!(
                "export truncated at {EXPORT_NODE_CAP} nodes; narrow the repo"
            )));
        }
        let rels = self
            .handle
            .build_relationships(&self.build_id, EXPORT_EDGE_CAP + 1)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        if i64::try_from(rels.len()).expect("relation count fits in i64") > EXPORT_EDGE_CAP {
            return Err(PipelineError::Consumer(format!(
                "export truncated at {EXPORT_EDGE_CAP} edges; narrow the repo"
            )));
        }
        let mut nodes: Vec<serde_json::Value> = entities
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.entity_id, "label": e.name, "kind": e.kind,
                    "source_file": e.file, "qualified_name": e.qualified_name,
                    "snapshot": e.snapshot, "span": e.span,
                    "compiler": e.compiler,
                })
            })
            .collect();
        nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let mut links: Vec<serde_json::Value> = rels
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.rel_id, "source": r.from_entity.entity_id,
                    "target": r.to_entity.entity_id, "rel_type": r.rel_type,
                })
            })
            .collect();
        links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(serde_json::json!({
            "directed": true, "multigraph": true,
            "nodes": nodes, "links": links,
            "build_id": self.build_id, "generation": self.generation,
            "snapshots": self.snapshots, "coverage": self.coverage.as_ref().map(chaosbox_core::coverage::BuildCoverage::report),
        }))
    }

    /// Claim evidence and source locations for one relationship of the
    /// pinned build.
    pub async fn evidence(&self, rel_id: &str) -> Result<serde_json::Value, PipelineError> {
        let rows = self
            .handle
            .evidence_for(&self.build_id, rel_id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        Ok(serde_json::json!({"rel": rel_id, "evidence": rows}))
    }

    /// Source-backed entity explanation: structured info, no generated prose.
    pub async fn explain(&self, id: &str) -> Result<serde_json::Value, PipelineError> {
        let entity = self
            .handle
            .entity_by_id(&self.build_id, id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        let Some(e) = entity else {
            return Ok(serde_json::Value::Null);
        };
        let (out, inc) = self
            .handle
            .neighbor_counts(&self.build_id, id, all_relation_types())
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        Ok(serde_json::json!({
            "id": e.entity_id, "kind": e.kind, "file": e.file,
            "qualified_name": e.qualified_name,
            "snapshot": e.snapshot, "span": e.span,
            "compiler": e.compiler,
            "outgoing": out, "incoming": inc,
        }))
    }

    /// Node/edge id diff between two builds of one repo, bounded by the
    /// export caps on each side.
    pub async fn diff(
        &self,
        repo: &str,
        from_build: &str,
        to_build: &str,
    ) -> Result<serde_json::Value, PipelineError> {
        use std::collections::BTreeSet;
        async fn members<R2: chaosbox_store::GraphQueries>(
            reader: &GraphReader<R2>,
            build: &str,
        ) -> Result<(BTreeSet<String>, BTreeSet<String>), PipelineError> {
            let ents = reader
                .handle
                .build_entities(build, EXPORT_NODE_CAP + 1)
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?;
            let rels = reader
                .handle
                .build_relationships(build, EXPORT_EDGE_CAP + 1)
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?;
            if i64::try_from(ents.len()).expect("entity count fits in i64") > EXPORT_NODE_CAP
                || i64::try_from(rels.len()).expect("relation count fits in i64") > EXPORT_EDGE_CAP
            {
                return Err(PipelineError::Consumer(
                    "diff truncated at export caps".into(),
                ));
            }
            Ok((
                ents.iter().map(|e| e.entity_id.clone()).collect(),
                rels.iter().map(|r| r.rel_id.clone()).collect(),
            ))
        }
        if repo != self.repo {
            return Err(PipelineError::Consumer(
                "repository differs from pinned reader".into(),
            ));
        }
        // Validate both headers before reading either side. Empty builds and
        // unknown ids cannot be authorized by examining their member rows.
        for build in [from_build, to_build] {
            if self
                .handle
                .published_build(repo, build)
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?
                .is_none()
            {
                return Err(PipelineError::Consumer(
                    "published build unavailable for repo".into(),
                ));
            }
        }
        let (old_n, old_e) = members(self, from_build).await?;
        let (new_n, new_e) = members(self, to_build).await?;
        let added_nodes: Vec<_> = new_n.difference(&old_n).cloned().collect();
        let removed_nodes: Vec<_> = old_n.difference(&new_n).cloned().collect();
        let added_edges: Vec<_> = new_e.difference(&old_e).cloned().collect();
        let removed_edges: Vec<_> = old_e.difference(&new_e).cloned().collect();
        Ok(serde_json::json!({
            "repo": repo, "from_build": from_build, "to_build": to_build,
            "added_nodes": added_nodes, "removed_nodes": removed_nodes,
            "added_edges": added_edges, "removed_edges": removed_edges,
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rank_fusion_rewards_agreement_and_keeps_a_lone_list_unchanged() {
        let ids = |v: &[&'static str]| v.to_vec();
        let lexical = ids(&["exact", "prefix", "substring"]);
        assert_eq!(super::rrf(vec![lexical.clone()], |s| s, 10), lexical);
        let fused = super::rrf(vec![lexical, ids(&["semantic", "substring"])], |s| s, 3);
        // Found by both lists outranks either list's sole first hit; ties
        // keep the lexical list first.
        assert_eq!(fused, ["substring", "exact", "semantic"]);
        assert!(super::rrf(vec![ids(&["a"])], |s| s, 0).is_empty());
    }
}
