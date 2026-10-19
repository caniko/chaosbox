//! Shared read-only graph queries (search, export, explain) for CLI and MCP.

use super::{GraphBuild, Entity, Relation};
use serde::{ser::SerializeMap, Serialize, Serializer};

// ---- Shared read-only queries (CLI and MCP use these) ----

/// Case-insensitive substring search over names. Bounded: sorts all matches
/// by qualified name, then takes the first `limit`.
#[must_use]
pub fn search(build: &GraphBuild, query: &str, limit: usize) -> Vec<Entity> {
    let q = query.to_lowercase();
    let mut out: Vec<Entity> = build
        .nodes
        .values()
        .filter(|e| {
            e.name.to_lowercase().contains(&q) || e.qualified_name.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    out.truncate(limit);
    out
}

/// Deterministic JSON export + Graphify-compatible node-link shape.
#[must_use]
pub fn export_json(build: &GraphBuild) -> serde_json::Value {
    let mut nodes: Vec<serde_json::Value> = build.nodes.values().map(export_node).collect();
    nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let mut links: Vec<serde_json::Value> = build.edges.values().map(export_edge).collect();
    links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    serde_json::json!({
        "directed": true, "multigraph": true,
        "nodes": nodes, "links": links,
        "build_id": build.id, "generation": build.generation,
        "snapshots": build.snapshot_ids, "coverage": build.coverage.as_ref().map(chaosbox_core::coverage::BuildCoverage::report),
    })
}

/// Write the same deterministic export as [`export_json`], materializing only
/// one node/edge JSON object at a time. The caller may buffer the output writer.
pub fn write_export_json(
    build: &GraphBuild,
    writer: impl std::io::Write,
) -> serde_json::Result<()> {
    let mut serializer = serde_json::Serializer::new(writer);
    let mut map = serializer.serialize_map(Some(8))?;
    // Match serde_json::Value's sorted object keys byte for byte.
    map.serialize_entry("build_id", &build.id)?;
    let coverage = serde_json::to_value(
        build
            .coverage
            .as_ref()
            .map(chaosbox_core::coverage::BuildCoverage::report),
    )?;
    map.serialize_entry("coverage", &coverage)?;
    map.serialize_entry("directed", &true)?;
    map.serialize_entry("generation", &build.generation)?;
    map.serialize_entry("links", &ExportEdges(build))?;
    map.serialize_entry("multigraph", &true)?;
    map.serialize_entry("nodes", &ExportNodes(build))?;
    map.serialize_entry("snapshots", &build.snapshot_ids)?;
    map.end()
}

struct ExportNodes<'a>(&'a GraphBuild);

impl Serialize for ExportNodes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut nodes: Vec<_> = self.0.nodes.values().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        serializer.collect_seq(nodes.into_iter().map(export_node))
    }
}

struct ExportEdges<'a>(&'a GraphBuild);

impl Serialize for ExportEdges<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut edges: Vec<_> = self.0.edges.values().collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        serializer.collect_seq(edges.into_iter().map(export_edge))
    }
}

fn export_node(e: &Entity) -> serde_json::Value {
    serde_json::json!({
        "id": e.id, "label": e.name, "kind": format!("{:?}", e.kind),
        "source_file": e.file,
        "source_location": format!("L{}", e.span.start_line),
        "qualified_name": e.qualified_name,
        "snapshot": e.snapshot, "span": e.span,
        "compiler": e.compiler,
    })
}

fn export_edge(r: &Relation) -> serde_json::Value {
    serde_json::json!({
        "id": r.id, "source": r.from, "target": r.to,
        "rel_type": format!("{:?}", r.rel_type),
        "scope": format!("{:?}", r.scope),
    })
}

/// Deterministic explain: source-backed structured info, no generated prose.
#[must_use]
pub fn explain_entity(build: &GraphBuild, id: &str) -> Option<serde_json::Value> {
    let e = build.nodes.get(id)?;
    Some(serde_json::json!({
        "id": e.id, "kind": format!("{:?}", e.kind),
        "file": e.file, "qualified_name": e.qualified_name,
        "span": e.span,
        "compiler": e.compiler,
        "outgoing": build.outgoing(id, None).len(),
        "incoming": build.incoming(id, None).len(),
    }))
}
