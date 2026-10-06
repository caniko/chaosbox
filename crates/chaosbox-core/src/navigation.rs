//! Deterministic, build-bound graph navigation. Connectivity groups are
//! weak components, not semantic communities or inferred architecture.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use serde_json::{json, Value};

type Index<'a> = BTreeMap<&'a str, &'a Value>;
type Adjacency<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

/// Publication-time analytics have a bounded projection independent of the
/// source graph's size. Full source/entity/evidence records are never cached.
pub const SUMMARY_BYTES_MAX: usize = 8 * 1024 * 1024;

/// Compute immutable weak-component statistics once while publishing a build.
/// The 200-entry/member limit matches the public analytics read contract.
pub fn summarize_build(build: &crate::GraphBuild) -> Result<Value, String> {
    let mut snapshots = build.snapshot_ids.clone();
    snapshots.sort();
    let graph = json!({
        "build_id":build.id,"generation":build.generation,"snapshots":snapshots,
        "coverage":build.coverage.as_ref().map(crate::coverage::BuildCoverage::report),
        "nodes":build.nodes.values().map(|e| json!({"id":e.id,"label":e.name,"source_file":e.file})).collect::<Vec<_>>(),
        "links":build.edges.values().map(|r| json!({"source":r.from,"target":r.to})).collect::<Vec<_>>(),
    });
    let packet = summary(&graph, 200)?;
    if serde_json::to_vec(&packet)
        .map_err(|_| "encode analytics")?
        .len()
        > SUMMARY_BYTES_MAX
    {
        return Err("publication analytics exceed byte budget".into());
    }
    Ok(packet)
}

fn index(graph: &Value) -> Result<(Index<'_>, Adjacency<'_>), String> {
    let mut nodes = BTreeMap::new();
    for node in graph["nodes"].as_array().ok_or("missing nodes")? {
        let id = node["id"].as_str().ok_or("node lacks identity")?;
        if nodes.insert(id, node).is_some() {
            return Err("duplicate node identity".into());
        }
    }
    let mut adjacency: Adjacency<'_> = nodes.keys().map(|id| (*id, BTreeSet::new())).collect();
    for edge in graph["links"].as_array().ok_or("missing links")? {
        let from = edge["source"].as_str().ok_or("edge lacks source")?;
        let to = edge["target"].as_str().ok_or("edge lacks target")?;
        adjacency.get_mut(from).ok_or("dangling source")?.insert(to);
        adjacency.get_mut(to).ok_or("dangling target")?.insert(from);
    }
    Ok((nodes, adjacency))
}

/// Statistics and degree-ranked hubs from a complete bounded export. Group
/// identities bind to the build and member ids; callers never treat them as
/// continuity across source changes.
pub fn summary(graph: &Value, limit: usize) -> Result<Value, String> {
    if !(1..=200).contains(&limit) {
        return Err("summary limit must be 1..200".into());
    }
    let (nodes, adjacency) = index(graph)?;
    let mut remaining: BTreeSet<_> = nodes.keys().copied().collect();
    let mut groups = Vec::new();
    while let Some(start) = remaining.pop_first() {
        let mut members = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(current) = queue.pop_front() {
            for next in &adjacency[current] {
                if remaining.remove(next) {
                    members.insert(*next);
                    queue.push_back(*next);
                }
            }
        }
        let encoded = serde_json::to_string(&members).map_err(|_| "encode group")?;
        groups.push(json!({
            "id":crate::deterministic_id("community", &[graph["build_id"].as_str().ok_or("missing build")?, &encoded]),
            "size":members.len(),"members":members.iter().take(limit).collect::<Vec<_>>(),
            "omitted_members":members.len().saturating_sub(limit),
        }));
    }
    groups.sort_by(|a, b| {
        b["size"]
            .as_u64()
            .cmp(&a["size"].as_u64())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let mut hubs: Vec<_> = nodes.iter().map(|(id,node)| json!({"id":id,"label":node["label"],"degree":adjacency[id].len(),"source_file":node["source_file"]})).collect();
    hubs.sort_by(|a, b| {
        b["degree"]
            .as_u64()
            .cmp(&a["degree"].as_u64())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let total_groups = groups.len();
    hubs.truncate(limit);
    groups.truncate(limit);
    Ok(
        json!({"build_id":graph["build_id"],"generation":graph["generation"],"snapshots":graph["snapshots"],
        "nodes":nodes.len(),"edges":graph["links"].as_array().ok_or("missing links")?.len(),
        "algorithm":"weak-connected-components-v1","community_count":total_groups,
        "communities":groups,"omitted_communities":total_groups.saturating_sub(limit),"hubs":hubs,
        "omitted_hubs":nodes.len().saturating_sub(limit),
        "coverage":graph["coverage"],"exhaustive":false}),
    )
}

/// Inspect a build-bound connectivity group from the bounded summary.
pub fn community(graph: &Value, id: &str, limit: usize) -> Result<Value, String> {
    if !(1..=200).contains(&limit) {
        return Err("community limit must be 1..200".into());
    }
    let stats = summary(graph, 200)?;
    let group = stats["communities"]
        .as_array()
        .ok_or("missing groups")?
        .iter()
        .find(|group| group["id"] == id)
        .ok_or("community is absent or outside the bounded summary")?;
    let members: BTreeSet<_> = group["members"]
        .as_array()
        .ok_or("missing members")?
        .iter()
        .take(limit)
        .filter_map(Value::as_str)
        .collect();
    let (index, _) = index(graph)?;
    let nodes: Vec<_> = members.iter().map(|id| index[id]).collect();
    Ok(
        json!({"build_id":graph["build_id"],"id":id,"algorithm":stats["algorithm"],"size":group["size"],
        "omitted_members":group["size"].as_u64().unwrap_or(0).saturating_sub(nodes.len() as u64),"nodes":nodes,"exhaustive":false}),
    )
}

/// Bounded lexical seeds followed by deterministic undirected BFS. All
/// returned edges keep their original direction, type and evidence identity.
pub fn context(
    graph: &Value,
    query: &str,
    depth: usize,
    max_nodes: usize,
    max_chars: usize,
) -> Result<Value, String> {
    validate_context(query, depth, max_nodes, max_chars)?;
    render_context(graph, query, depth, max_nodes, max_chars)
}

/// Validate the shared CLI/MCP context budget before performing backend reads.
pub fn validate_context(
    query: &str,
    depth: usize,
    max_nodes: usize,
    max_chars: usize,
) -> Result<(), String> {
    if query.trim().is_empty()
        || query.len() > 2048
        || !(1..=6).contains(&depth)
        || !(1..=200).contains(&max_nodes)
        || !(256..=32_000).contains(&max_chars)
    {
        return Err(
            "context requires query <=2048 bytes, depth 1..6, nodes 1..200, chars 256..32000"
                .into(),
        );
    }
    Ok(())
}

fn render_context(
    graph: &Value,
    query: &str,
    depth: usize,
    max_nodes: usize,
    max_chars: usize,
) -> Result<Value, String> {
    let (nodes, adjacency) = index(graph)?;
    let query = query.to_lowercase();
    let terms: BTreeSet<_> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .collect();
    let mut seeds: Vec<_> = nodes
        .iter()
        .filter_map(|(id, node)| {
            let name = format!(
                "{} {}",
                node["label"].as_str().unwrap_or(""),
                node["qualified_name"].as_str().unwrap_or("")
            )
            .to_lowercase();
            let score = terms.iter().filter(|term| name.contains(**term)).count();
            (score > 0).then_some((score, *id))
        })
        .collect();
    seeds.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let mut selected = BTreeSet::new();
    let mut priority = Vec::new();
    let mut queue = VecDeque::new();
    let mut truncated = seeds.len() > max_nodes;
    for (_, id) in seeds.into_iter().take(max_nodes) {
        selected.insert(id);
        priority.push(id);
        queue.push_back((id, 0));
    }
    while let Some((id, level)) = queue.pop_front() {
        if level >= depth {
            continue;
        }
        for next in &adjacency[id] {
            if selected.contains(next) {
                continue;
            }
            if selected.len() == max_nodes {
                truncated = true;
                continue;
            }
            selected.insert(*next);
            priority.push(*next);
            queue.push_back((*next, level + 1));
        }
    }
    loop {
        let mut links: Vec<_> = graph["links"]
            .as_array()
            .ok_or("missing links")?
            .iter()
            .filter(|edge| {
                edge["source"]
                    .as_str()
                    .is_some_and(|id| selected.contains(id))
                    && edge["target"]
                        .as_str()
                        .is_some_and(|id| selected.contains(id))
            })
            .collect();
        links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let packet = json!({"build_id":graph["build_id"],"generation":graph["generation"],"snapshots":graph["snapshots"],
            "algorithm":"lexical-bfs-v1","direction":"both","nodes":selected.iter().map(|id| nodes[id]).collect::<Vec<_>>(),
            "links":links,"truncated":truncated,"exhaustive":false});
        if serde_json::to_string(&packet)
            .map_err(|_| "encode context")?
            .chars()
            .count()
            <= max_chars
        {
            return Ok(packet);
        }
        let Some(last) = priority.pop() else {
            return Err("context metadata exceeds output budget".into());
        };
        selected.remove(last);
        truncated = true;
    }
}
