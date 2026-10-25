//! Backend-bounded context and publication-cached analytics.

use std::collections::{BTreeMap, VecDeque};

use chaosbox_store::{EntityRow, GraphQueries, RelRow};
use serde_json::{Value, json};

use super::{GraphReader, PipelineError, escape_like};

const CONTEXT_EDGE_CAP: usize = 2_000;
const CONTEXT_TERM_CAP: usize = 32;

#[derive(Default)]
struct ContextGraph {
    nodes: BTreeMap<String, EntityRow>,
    edges: BTreeMap<String, RelRow>,
    priority: Vec<String>,
    edge_reads: usize,
    truncated: bool,
    /// Whether embedded nearest neighbours contributed seeds.
    semantic: bool,
}

impl ContextGraph {
    async fn seed<R: GraphQueries>(
        reader: &GraphReader<R>,
        query: &str,
        max_nodes: usize,
    ) -> Result<Self, PipelineError> {
        // Short terms and function words ("a", "is", "the") match nearly every
        // label; Graphify drops them, and so do we unless nothing remains.
        let terms = chaosbox_core::navigation::context_terms(query);
        if terms.len() > CONTEXT_TERM_CAP {
            return Err(PipelineError::Consumer(
                "context term budget exceeded; narrow the query".into(),
            ));
        }
        let mut graph = Self::default();
        let mut candidates = BTreeMap::new();
        for term in &terms {
            let rows = reader
                .handle
                .search_entities(
                    &reader.build_id,
                    &format!("%{}%", escape_like(term)),
                    i64::try_from(max_nodes + 1).unwrap_or(201),
                )
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?;
            graph.truncated |= rows.len() > max_nodes;
            for row in rows {
                candidates.insert(row.entity_id.clone(), row);
            }
        }
        // Name hits weigh double qualified-name/path hits (Graphify: label 1.0,
        // source file 0.5).
        let score = |e: &EntityRow| {
            let (name, qualified) = (e.name.to_lowercase(), e.qualified_name.to_lowercase());
            terms
                .iter()
                .map(|term| {
                    2 * usize::from(name.contains(term.as_str()))
                        + usize::from(qualified.contains(term.as_str()))
                })
                .sum::<usize>()
        };
        let mut candidates: Vec<_> = candidates.into_values().collect();
        candidates.sort_by(|a, b| {
            score(b)
                .cmp(&score(a))
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        graph.truncated |= candidates.len() > max_nodes;
        let mut lists = vec![candidates];
        if let Some(nearest) = reader.semantic(query, max_nodes).await {
            graph.semantic = true;
            lists.push(nearest);
        }
        let candidates = super::rrf(lists, |e| e.entity_id.as_str(), max_nodes);
        for row in candidates {
            graph.priority.push(row.entity_id.clone());
            graph.nodes.insert(row.entity_id.clone(), row);
        }
        Ok(graph)
    }

    async fn expand<R: GraphQueries>(
        &mut self,
        reader: &GraphReader<R>,
        depth: usize,
        max_nodes: usize,
    ) -> Result<(), PipelineError> {
        let mut queue: VecDeque<_> = self.priority.iter().cloned().map(|id| (id, 0)).collect();
        while let Some((id, level)) = queue.pop_front() {
            if level >= depth {
                continue;
            }
            let remaining = CONTEXT_EDGE_CAP.saturating_sub(self.edge_reads);
            if remaining == 0 {
                self.truncated = true;
                break;
            }
            let rows = reader
                .handle
                .adjacent_relationships(
                    &reader.build_id,
                    &id,
                    i64::try_from(remaining + 1).unwrap_or(2001),
                )
                .await
                .map_err(|e| PipelineError::Consumer(e.to_string()))?;
            self.truncated |= rows.len() > remaining;
            self.edge_reads += rows.len().min(remaining);
            for row in rows.into_iter().take(remaining) {
                let next = if row.from_entity.entity_id == id {
                    &row.to_entity.entity_id
                } else {
                    &row.from_entity.entity_id
                };
                if !self.nodes.contains_key(next) {
                    if self.nodes.len() < max_nodes {
                        let entity = reader.lookup(next).await?.ok_or_else(|| {
                            PipelineError::Consumer("dangling context endpoint".into())
                        })?;
                        self.nodes.insert(next.clone(), entity);
                        self.priority.push(next.clone());
                        queue.push_back((next.clone(), level + 1));
                    } else {
                        self.truncated = true;
                    }
                }
                self.edges.insert(row.rel_id.clone(), row);
            }
        }
        Ok(())
    }

    fn render<R>(
        &mut self,
        reader: &GraphReader<R>,
        max_chars: usize,
    ) -> Result<Value, PipelineError> {
        loop {
            let algorithm = if self.semantic {
                "bounded-hybrid-bfs-v1"
            } else {
                "bounded-lexical-bfs-v3"
            };
            let packet = json!({
                "build_id":reader.build_id,"generation":reader.generation,"snapshots":reader.snapshots,
                "algorithm":algorithm,"semantic":self.semantic,"direction":"both","truncated":self.truncated,"exhaustive":false,
                "nodes":self.nodes.values().map(entity_projection).collect::<Vec<_>>(),
                "links":self.edges.values().filter(|r| self.nodes.contains_key(&r.from_entity.entity_id) && self.nodes.contains_key(&r.to_entity.entity_id))
                    .map(|r| json!({"id":r.rel_id,"source":r.from_entity.entity_id,"target":r.to_entity.entity_id,"rel_type":r.rel_type})).collect::<Vec<_>>(),
            });
            if packet.to_string().chars().count() <= max_chars {
                return Ok(packet);
            }
            let id = self.priority.pop().ok_or_else(|| {
                PipelineError::Consumer("context metadata exceeds output budget".into())
            })?;
            self.nodes.remove(&id);
            self.truncated = true;
        }
    }
}

fn entity_projection(e: &EntityRow) -> Value {
    json!({"id":e.entity_id,"label":e.name,"kind":e.kind,"source_file":e.file,
        "qualified_name":e.qualified_name,"snapshot":e.snapshot,"span":e.span,"compiler":e.compiler})
}

impl<R: GraphQueries> GraphReader<R> {
    /// Bounded lexical seeds and undirected expansion of this pinned build.
    /// Saturated seed/neighborhood queries mark the result truncated.
    pub async fn context(
        &self,
        query: &str,
        depth: usize,
        max_nodes: usize,
        max_chars: usize,
    ) -> Result<Value, PipelineError> {
        chaosbox_core::navigation::validate_context(query, depth, max_nodes, max_chars)
            .map_err(PipelineError::Consumer)?;
        let mut graph = ContextGraph::seed(self, query, max_nodes).await?;
        graph.expand(self, depth, max_nodes).await?;
        graph.render(self, max_chars)
    }

    /// Publication analytics at `resolution`. The stored packet serves the
    /// default resolution while its algorithm is current; anything else is
    /// recomputed from the complete bounded export (Graphify's
    /// `--cluster-only`/`--resolution`, with no stored state). A stale packet
    /// whose build exceeds the export cap is served as-is with a refresh hint.
    async fn navigation_packet(&self, resolution: f64) -> Result<Value, PipelineError> {
        use chaosbox_core::navigation::{summary_at, ALGORITHM, DEFAULT_RESOLUTION};
        if !(0.01..=10.0).contains(&resolution) {
            return Err(PipelineError::Consumer(
                "resolution must be 0.01..10".into(),
            ));
        }
        let stored = self
            .handle
            .navigation_summary(&self.build_id)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        #[allow(clippy::float_cmp)] // exact default, never a computed value
        let default = resolution == DEFAULT_RESOLUTION;
        let packet = match stored {
            Some(packet) if default && packet["algorithm"] == ALGORITHM => packet,
            stale => match self.export().await {
                Ok(graph) => {
                    summary_at(&graph, 200, resolution).map_err(PipelineError::Consumer)?
                }
                Err(error) => match stale.filter(|_| default) {
                    Some(mut packet) => {
                        packet["refresh"] =
                            json!("analytics predate this reader; republish with `chaosbox run`");
                        packet
                    }
                    None => return Err(error),
                },
            },
        };
        if packet["build_id"] != self.build_id
            || packet["generation"] != self.generation
            || packet["snapshots"] != json!(self.snapshots)
        {
            return Err(PipelineError::Consumer(
                "analytics do not match the pinned build".into(),
            ));
        }
        Ok(packet)
    }

    /// [`Self::stats_at`] at the default resolution.
    pub async fn stats(&self, limit: usize) -> Result<Value, PipelineError> {
        self.stats_at(limit, chaosbox_core::navigation::DEFAULT_RESOLUTION)
            .await
    }

    /// Counts, Leiden communities, hubs, god nodes, surprising connections and
    /// answerable questions for the pinned build, bounded to `limit` entries.
    pub async fn stats_at(&self, limit: usize, resolution: f64) -> Result<Value, PipelineError> {
        if !(1..=200).contains(&limit) {
            return Err(PipelineError::Consumer(
                "summary limit must be 1..200".into(),
            ));
        }
        let mut packet = self.navigation_packet(resolution).await?;
        let invalid = |what: &str| PipelineError::Consumer(format!("invalid analytics {what}"));
        for list in ["hubs", "god_nodes"] {
            if let Some(items) = packet[list].as_array_mut() {
                items.truncate(limit);
            }
        }
        packet["omitted_hubs"] = json!(packet["nodes"]
            .as_u64()
            .ok_or_else(|| invalid("node count"))?
            .saturating_sub(limit as u64));
        let total = packet["community_count"]
            .as_u64()
            .ok_or_else(|| invalid("group count"))?;
        let groups = packet["communities"]
            .as_array_mut()
            .ok_or_else(|| invalid("groups"))?;
        groups.truncate(limit);
        for group in groups {
            let size = group["size"]
                .as_u64()
                .ok_or_else(|| invalid("group size"))?;
            let members = group["members"]
                .as_array_mut()
                .ok_or_else(|| invalid("members"))?;
            members.truncate(limit);
            group["omitted_members"] = json!(size.saturating_sub(members.len() as u64));
        }
        packet["omitted_communities"] = json!(total.saturating_sub(limit as u64));
        // Questions name the exact read that answers them, for this repository.
        if let Some(questions) = packet["questions"].as_array_mut() {
            for question in questions {
                question["args"]["repo"] = json!(self.repo);
            }
        }
        Ok(packet)
    }

    /// [`Self::community_at`] at the default resolution.
    pub async fn community(&self, id: &str, limit: usize) -> Result<Value, PipelineError> {
        self.community_at(id, limit, chaosbox_core::navigation::DEFAULT_RESOLUTION)
            .await
    }

    /// Inspect one community of the pinned build's analytics.
    pub async fn community_at(
        &self,
        id: &str,
        limit: usize,
        resolution: f64,
    ) -> Result<Value, PipelineError> {
        if !(1..=200).contains(&limit) {
            return Err(PipelineError::Consumer(
                "community limit must be 1..200".into(),
            ));
        }
        let packet = self.navigation_packet(resolution).await?;
        let group = packet["communities"]
            .as_array()
            .and_then(|groups| groups.iter().find(|group| group["id"] == id))
            .ok_or_else(|| {
                PipelineError::Consumer("community is absent or outside the bounded summary".into())
            })?;
        let members = group["members"]
            .as_array()
            .ok_or_else(|| PipelineError::Consumer("invalid analytics members".into()))?;
        let mut nodes = Vec::new();
        for member in members.iter().take(limit) {
            let entity = self
                .lookup(
                    member.as_str().ok_or_else(|| {
                        PipelineError::Consumer("invalid analytics member".into())
                    })?,
                )
                .await?
                .ok_or_else(|| PipelineError::Consumer("dangling analytics member".into()))?;
            nodes.push(entity_projection(&entity));
        }
        Ok(
            json!({"build_id":self.build_id,"generation":self.generation,"snapshots":self.snapshots,
            "id":id,"algorithm":packet["algorithm"],"label":group["label"],"cohesion":group["cohesion"],
            "size":group["size"],"nodes":nodes,
            "omitted_members":group["size"].as_u64().unwrap_or(0).saturating_sub(nodes.len() as u64),"exhaustive":false}),
        )
    }

    /// Markdown report (Graphify's `GRAPH_REPORT.md`) from the same packet as
    /// [`Self::stats_at`], citing ids with the commands that verify them.
    pub async fn report(&self, resolution: f64) -> Result<String, PipelineError> {
        let packet = self.stats_at(20, resolution).await?;
        Ok(chaosbox_core::report::markdown(&packet, &self.repo))
    }
}
