//! Read-only consumer queries: LIKE escaping, relation filters, and the pinned-graph reader.

use super::PipelineError;

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

/// Backend read-only queries. One build id is pinned per reader from the
/// active-build pointer; readers never mutate, migrate, or load Jev credentials.
/// Generic over [`chaosbox_store::GraphQueries`] with `TypeDbReader` as the
/// live backend and [`chaosbox_store::MemoryReader`] for tests.
pub struct GraphReader<R> {
    handle: R,
    /// Pinned active build id for every request this reader serves.
    pub build_id: String,
    /// Pinned generation (predecessor/generation checks on the read side).
    pub generation: i64,
    /// Pinned build status (`active`; the pointer can only pin active builds).
    pub status: String,
    /// Snapshot ids pinned by the build: the freshness fingerprint status
    /// reports so consumers can detect a build that no longer matches its
    /// sources. Empty when the backend's projection predates the field.
    pub snapshots: Vec<String>,
}

impl<R: chaosbox_store::GraphQueries> GraphReader<R> {
    /// Pin the active build for `repo` on an existing query backend.
    /// Used by tests with [`chaosbox_store::MemoryReader`].
    pub async fn pinned(handle: R, repo: &str) -> Result<Self, PipelineError> {
        let build = handle
            .active_build(repo)
            .await
            .map_err(|e| PipelineError::Consumer(format!("active build: {e}")))?
            .ok_or_else(|| PipelineError::Consumer(format!("no active build for repo {repo}")))?;
        Ok(Self {
            handle,
            build_id: build.build_id,
            generation: build.generation,
            status: build.status,
            snapshots: build.snapshots,
        })
    }

    /// Bounded substring search over the pinned build's entity names.
    /// LIKE wildcards in the query are escaped: they match literally.
    pub async fn search(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<chaosbox_store::EntityRow>, PipelineError> {
        let like = format!("%{}%", escape_like(query));
        self.handle
            .search_entities(&self.build_id, &like, limit)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))
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

    /// Upper bound on BFS node visits for one path query. Exceeding it is
    /// an explicit budget error, never an unbounded traversal: callers
    /// (MCP agents) retry with a narrower query instead of hanging the
    /// serial server loop.
    pub const PATH_VISITED_CAP: usize = 100_000;

    /// Bounded BFS path using iterative backend neighborhood expansion.
    /// `ponytail: O(hops * degree) round-trips; single-projection fetch if this dominates`.
    pub async fn path(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        self.path_with_cap(from, to, max_hops, Self::PATH_VISITED_CAP)
            .await
    }

    /// [`GraphReader::path`] with an explicit visit budget (tests + future policy).
    pub async fn path_with_cap(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
        visit_cap: usize,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        if from == to {
            return Ok(Some(vec![from.to_owned()]));
        }
        let types = all_relation_types();
        let mut prev: BTreeMap<String, String> = BTreeMap::new();
        let mut seen: BTreeSet<String> = BTreeSet::from([from.to_owned()]);
        let mut queue: VecDeque<(String, usize)> = VecDeque::from([(from.to_owned(), 0)]);
        while let Some((cur, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            let (out, inc) = self.neighbors(&cur, Some(types.clone())).await?;
            let mut nexts: Vec<String> = Vec::new();
            for r in out.iter().chain(inc.iter()) {
                nexts.push(r.from_entity.entity_id.clone());
                nexts.push(r.to_entity.entity_id.clone());
            }
            for nxt in nexts {
                if nxt == cur || !seen.insert(nxt.clone()) {
                    continue;
                }
                if seen.len() > visit_cap {
                    return Err(PipelineError::Consumer(
                        "path traversal budget exceeded; narrow the query".into(),
                    ));
                }
                prev.insert(nxt.clone(), cur.clone());
                if nxt == to {
                    let mut path = vec![to.to_owned()];
                    let mut c = to.to_owned();
                    while let Some(p) = prev.get(&c) {
                        path.push(p.clone());
                        c = p.clone();
                    }
                    path.reverse();
                    return Ok(Some(path));
                }
                queue.push_back((nxt, depth + 1));
            }
        }
        Ok(None)
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
        let (out, inc) = self.neighbors(id, None).await?;
        Ok(serde_json::json!({
            "id": e.entity_id, "kind": e.kind, "file": e.file,
            "qualified_name": e.qualified_name,
            "outgoing": out.len(), "incoming": inc.len(),
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
