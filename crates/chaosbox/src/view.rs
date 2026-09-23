//! Shared read-only graph queries (search, export, explain) for CLI and MCP.

use super::{GraphBuild, Entity};

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
    let mut nodes: Vec<serde_json::Value> = build
        .nodes
        .values()
        .map(|e| {
            serde_json::json!({
                "id": e.id, "label": e.name, "kind": format!("{:?}", e.kind),
                "source_file": e.file,
                "source_location": format!("L{}", e.span.start_line),
                "qualified_name": e.qualified_name,
            })
        })
        .collect();
    nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let mut links: Vec<serde_json::Value> = build
        .edges
        .values()
        .map(|r| {
            serde_json::json!({
                "id": r.id, "source": r.from, "target": r.to,
                "rel_type": format!("{:?}", r.rel_type),
                "scope": format!("{:?}", r.scope),
            })
        })
        .collect();
    links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    serde_json::json!({
        "directed": true, "multigraph": true,
        "nodes": nodes, "links": links,
        "build_id": build.id, "generation": build.generation,
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
        "outgoing": build.outgoing(id, None).len(),
        "incoming": build.incoming(id, None).len(),
    }))
}
