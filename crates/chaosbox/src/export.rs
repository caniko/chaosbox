//! Offline interop renderings of a node-link export: self-contained HTML,
//! Mermaid, `GraphML`, Cypher and a Markdown wiki. Pure, deterministic
//! functions over the bounded reader export plus its Leiden communities;
//! every label is untrusted source text and is escaped for its target.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chaosbox_core::navigation::Community;
use serde_json::{json, Value};

/// Node-level views above this many nodes collapse into one node per
/// community, so large builds stay readable instead of being refused.
pub const HTML_NODE_LIMIT: usize = 1_000;
/// Mermaid diagrams stop being legible far earlier than HTML.
pub const MERMAID_NODE_LIMIT: usize = 150;
/// Communities drawn by the collapsed HTML overview, largest first.
const HTML_COMMUNITY_LIMIT: usize = 300;
/// Containment and declaration edges are everywhere; diagrams omit them.
const PLUMBING: [&str; 2] = ["contains", "defines"];

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

/// `query export --format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Node-link JSON (Graphify-compatible outline).
    Json,
    /// Self-contained interactive HTML.
    Html,
    /// Mermaid flowchart in a fenced Markdown block.
    Mermaid,
    /// `GraphML` (Gephi, yEd, Cytoscape).
    Graphml,
    /// Cypher statements for Neo4j import.
    Cypher,
    /// Markdown wiki / Obsidian vault (directory output).
    Wiki,
}

/// Render the bounded export, optionally narrowed to one community (id from
/// `stats`) or to a query's bounded lexical context. Returns `(file name,
/// contents)`; single-file formats use an empty name.
pub fn render(
    graph: &Value,
    format: Format,
    community: Option<&str>,
    query: Option<&str>,
    resolution: f64,
    repo: &str,
) -> Result<Vec<(String, String)>, String> {
    // Canonical order: identical bytes whatever order the export arrived in.
    let mut graph = graph.clone();
    for list in ["nodes", "links"] {
        if let Some(items) = graph[list].as_array_mut() {
            items.sort_by(|a, b| str_of(a, "id").cmp(str_of(b, "id")));
        }
    }
    let graph = &graph;
    let communities = chaosbox_core::navigation::communities(graph, resolution)?;
    let graph = if let Some(id) = community {
        let found = communities
            .iter()
            .find(|c| c.id == id)
            .ok_or("community is absent from this build at this resolution")?;
        select(graph, &found.members.iter().cloned().collect())
    } else if let Some(query) = query {
        let context = chaosbox_core::navigation::context(graph, query, 2, 200, 32_000)?;
        let ids = context["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n["id"].as_str().map(str::to_owned))
            .collect();
        select(graph, &ids)
    } else {
        graph.clone()
    };
    let single = |text: String| vec![(String::new(), text)];
    Ok(match format {
        Format::Json => single(format!("{graph}\n")),
        Format::Html => single(html(
            &graph,
            &communities,
            &format!("{repo} · {}", str_of(&graph, "build_id")),
        )),
        Format::Mermaid => single(mermaid(&graph, &communities)),
        Format::Graphml => single(graphml(&graph, &communities)),
        Format::Cypher => single(cypher(&graph, &communities)),
        Format::Wiki => wiki(&graph, &communities, repo),
    })
}

/// Node id -> community rank.
fn ranks(communities: &[Community]) -> BTreeMap<&str, usize> {
    communities
        .iter()
        .enumerate()
        .flat_map(|(rank, c)| c.members.iter().map(move |m| (m.as_str(), rank)))
        .collect()
}

/// Restrict a node-link graph to `ids` and the links among them.
#[must_use]
pub fn select(graph: &Value, ids: &BTreeSet<String>) -> Value {
    let mut out = graph.clone();
    let keep = |v: &Value, key: &str| v[key].as_str().is_some_and(|id| ids.contains(id));
    if let Some(nodes) = out["nodes"].as_array_mut() {
        nodes.retain(|n| keep(n, "id"));
    }
    if let Some(links) = out["links"].as_array_mut() {
        links.retain(|l| keep(l, "source") && keep(l, "target"));
    }
    out
}

/// One node per community, links weighted by crossing relationships. Only
/// the `max` largest multi-member communities are drawn; singletons (mostly
/// unresolved imports and mentions) are counted, not drawn.
fn collapse(graph: &Value, communities: &[Community], max: usize) -> Value {
    let rank = ranks(communities);
    let mut weights: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for link in graph["links"].as_array().into_iter().flatten() {
        let (Some(a), Some(b)) = (
            rank.get(str_of(link, "source")),
            rank.get(str_of(link, "target")),
        ) else {
            continue;
        };
        if a != b {
            *weights.entry((*a.min(b), *a.max(b))).or_default() += 1;
        }
    }
    let present: BTreeSet<usize> = graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| rank.get(str_of(n, "id")).copied())
        .filter(|&c| communities[c].members.len() > 1)
        .collect();
    // Ranks ascend from the largest community.
    let present: BTreeSet<usize> = present.into_iter().take(max).collect();
    json!({
        "nodes": present.iter().map(|&c| json!({"id":communities[c].id,
            "label":format!("{} ({})", communities[c].label, communities[c].members.len()),
            "kind":"community","community":c,"size":communities[c].members.len()})).collect::<Vec<_>>(),
        "links": weights.iter().filter(|((a, b), _)| present.contains(a) && present.contains(b))
            .map(|((a, b), w)| json!({"id":format!("{a}-{b}"),"source":communities[*a].id,
                "target":communities[*b].id,"rel_type":"crosses","weight":w}))
            .collect::<Vec<_>>(),
        "omitted_communities": communities.len() - present.len(),
    })
}

/// JSON safe inside `<script type="application/json">`: no `<`, `>`, `&` or
/// JavaScript line terminators survive, so labels cannot close the element.
fn script_json(value: &Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Self-contained interactive HTML (no network: `default-src 'none'`).
#[must_use]
pub fn html(graph: &Value, communities: &[Community], title: &str) -> String {
    let collapsed = graph["nodes"].as_array().map_or(0, Vec::len) > HTML_NODE_LIMIT;
    let view = if collapsed {
        collapse(graph, communities, HTML_COMMUNITY_LIMIT)
    } else {
        let rank = ranks(communities);
        let mut view = json!({"nodes":[],"links":graph["links"]});
        view["nodes"] = graph["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|n| {
                let c = rank.get(str_of(n, "id")).copied().unwrap_or(0);
                json!({"id":n["id"],"label":n["label"],"kind":n["kind"],
                    "file":n["source_file"],"line":n["source_location"],"community":c,
                    "community_label":communities.get(c).map(|c| c.label.as_str())})
            })
            .collect();
        view
    };
    let data = json!({"title":title,"collapsed":collapsed,"graph":view});
    include_str!("export.html")
        .replace("{{TITLE}}", &xml(title))
        .replace("{{DATA}}", &script_json(&data))
}

/// Mermaid `flowchart` in a fenced block, rendered natively by GitHub,
/// Forgejo and Obsidian. Shows non-plumbing relationships; large graphs
/// collapse to the community graph.
#[must_use]
pub fn mermaid(graph: &Value, communities: &[Community]) -> String {
    let collapsed = graph["nodes"].as_array().map_or(0, Vec::len) > MERMAID_NODE_LIMIT;
    let view = if collapsed {
        collapse(graph, communities, MERMAID_NODE_LIMIT)
    } else {
        graph.clone()
    };
    let links: Vec<&Value> = view["links"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| !PLUMBING.contains(&str_of(l, "rel_type")))
        .collect();
    let shown: BTreeSet<&str> = links
        .iter()
        .flat_map(|l| [str_of(l, "source"), str_of(l, "target")])
        .collect();
    let nodes: Vec<&Value> = view["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| collapsed || shown.contains(str_of(n, "id")))
        .collect();
    let index: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (str_of(n, "id"), i))
        .collect();
    let mut out = String::from("```mermaid\nflowchart LR\n");
    let total = view["nodes"].as_array().map_or(0, Vec::len);
    if nodes.len() < total {
        let _ = writeln!(
            out,
            "  %% {} nodes without shown relationships omitted",
            total - nodes.len()
        );
    }
    for (i, node) in nodes.iter().enumerate() {
        let _ = writeln!(out, "  n{i}[\"{}\"]", mermaid_text(str_of(node, "label")));
    }
    for link in links {
        if let (Some(a), Some(b)) = (
            index.get(str_of(link, "source")),
            index.get(str_of(link, "target")),
        ) {
            let label = link["weight"]
                .as_u64()
                .map_or_else(|| str_of(link, "rel_type").to_owned(), |w| w.to_string());
            let _ = writeln!(out, "  n{a} -->|\"{}\"| n{b}", mermaid_text(&label));
        }
    }
    out.push_str("```\n");
    out
}

/// Mermaid label text: anything outside a conservative set becomes an
/// entity code (`#35;`), and labels are capped at 60 characters.
fn mermaid_text(text: &str) -> String {
    text.chars()
        .take(60)
        .map(|c| {
            if c.is_ascii_alphanumeric() || " _.:/()-,".contains(c) {
                c.to_string()
            } else {
                format!("#{};", u32::from(c))
            }
        })
        .collect()
}

/// XML text/attribute escaping; characters XML 1.0 forbids are dropped.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if u32::from(c) < 0x20 || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

/// `GraphML` for Gephi, yEd and Cytoscape.
#[must_use]
pub fn graphml(graph: &Value, communities: &[Community]) -> String {
    let rank = ranks(communities);
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">\n",
    );
    for (key, target, kind) in [
        ("label", "node", "string"),
        ("kind", "node", "string"),
        ("source_file", "node", "string"),
        ("source_location", "node", "string"),
        ("community", "node", "int"),
        ("community_label", "node", "string"),
        ("rel_type", "edge", "string"),
        ("scope", "edge", "string"),
    ] {
        let _ = writeln!(
            out,
            "  <key id=\"{key}\" for=\"{target}\" attr.name=\"{key}\" attr.type=\"{kind}\"/>"
        );
    }
    let _ = writeln!(
        out,
        "  <graph id=\"{}\" edgedefault=\"directed\">",
        xml(str_of(graph, "build_id"))
    );
    for node in graph["nodes"].as_array().into_iter().flatten() {
        let id = str_of(node, "id");
        let _ = write!(out, "    <node id=\"{}\">", xml(id));
        for key in ["label", "kind", "source_file", "source_location"] {
            let _ = write!(out, "<data key=\"{key}\">{}</data>", xml(str_of(node, key)));
        }
        if let Some(&c) = rank.get(id) {
            let _ = write!(
                out,
                "<data key=\"community\">{c}</data><data key=\"community_label\">{}</data>",
                xml(&communities[c].label)
            );
        }
        out.push_str("</node>\n");
    }
    for link in graph["links"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "    <edge id=\"{}\" source=\"{}\" target=\"{}\"><data key=\"rel_type\">{}</data><data key=\"scope\">{}</data></edge>",
            xml(str_of(link, "id")),
            xml(str_of(link, "source")),
            xml(str_of(link, "target")),
            xml(str_of(link, "rel_type")),
            xml(str_of(link, "scope")),
        );
    }
    out.push_str("  </graph>\n</graphml>\n");
    out
}

/// Cypher string literal (single-quoted).
fn cypher_text(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Idempotent Cypher for Neo4j import (`cypher-shell < graph.cypher`).
/// Relationship types come from the closed vocabulary, uppercased.
#[must_use]
pub fn cypher(graph: &Value, communities: &[Community]) -> String {
    let rank = ranks(communities);
    let mut out = String::from(
        "CREATE CONSTRAINT chaosbox_entity_id IF NOT EXISTS FOR (n:Entity) REQUIRE n.id IS UNIQUE;\n",
    );
    let build = cypher_text(str_of(graph, "build_id"));
    for node in graph["nodes"].as_array().into_iter().flatten() {
        let id = str_of(node, "id");
        let _ = write!(
            out,
            "MERGE (n:Entity {{id: {}}}) SET n.label = {}, n.kind = {}, n.source_file = {}, n.source_location = {}, n.build_id = {build}",
            cypher_text(id),
            cypher_text(str_of(node, "label")),
            cypher_text(str_of(node, "kind")),
            cypher_text(str_of(node, "source_file")),
            cypher_text(str_of(node, "source_location")),
        );
        if let Some(&c) = rank.get(id) {
            let _ = write!(
                out,
                ", n.community = {c}, n.community_label = {}",
                cypher_text(&communities[c].label)
            );
        }
        out.push_str(";\n");
    }
    for link in graph["links"].as_array().into_iter().flatten() {
        let label: String = str_of(link, "rel_type")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
            .to_ascii_uppercase();
        let label = if label.is_empty() {
            "RELATED".to_owned()
        } else {
            label
        };
        let _ = writeln!(
            out,
            "MATCH (a:Entity {{id: {}}}), (b:Entity {{id: {}}}) MERGE (a)-[r:{label} {{id: {}}}]->(b) SET r.scope = {};",
            cypher_text(str_of(link, "source")),
            cypher_text(str_of(link, "target")),
            cypher_text(str_of(link, "id")),
            cypher_text(str_of(link, "scope")),
        );
    }
    out
}

/// Markdown table cell / inline code safe text.
fn md(text: &str) -> String {
    text.replace('|', "\\|")
        .replace('`', "'")
        .replace(['\n', '\r'], " ")
}

fn note_name(rank: usize) -> String {
    format!("community-{rank:03}.md")
}

/// One community's wiki page: members, connected communities and its
/// non-plumbing internal relationships.
fn community_page(
    c: usize,
    communities: &[Community],
    nodes: &BTreeMap<&str, &Value>,
    crossing: Option<&BTreeMap<usize, usize>>,
    internal: &[&Value],
) -> String {
    let community = &communities[c];
    let linked = |c: usize| communities[c].members.len() > 1;
    let mut page = format!(
        "# {}\n\n[Index](index.md) · {} members · id `{}`\n\n## Members\n\n| Name | Kind | Location |\n| --- | --- | --- |\n",
        md(&community.label),
        community.members.len(),
        md(&community.id)
    );
    for member in &community.members {
        let node = nodes.get(member.as_str()).copied().unwrap_or(&Value::Null);
        let _ = writeln!(
            page,
            "| `{}` | {} | {}:{} |",
            md(str_of(node, "label")),
            md(str_of(node, "kind")),
            md(str_of(node, "source_file")),
            md(str_of(node, "source_location"))
        );
    }
    if let Some(neighbors) = crossing {
        page.push_str("\n## Connected communities\n\n");
        for (other, count) in neighbors {
            let target = if linked(*other) {
                note_name(*other)
            } else {
                "index.md".to_owned()
            };
            let _ = writeln!(
                page,
                "- [{}]({target}) — {count} relationships",
                md(&communities[*other].label)
            );
        }
    }
    let shown: Vec<&&Value> = internal
        .iter()
        .filter(|l| !PLUMBING.contains(&str_of(l, "rel_type")))
        .take(200)
        .collect();
    if !shown.is_empty() {
        page.push_str("\n## Relationships\n\n");
        for link in shown {
            let name = |key: &str| {
                nodes
                    .get(str_of(link, key))
                    .map_or("", |n| str_of(n, "label"))
            };
            let _ = writeln!(
                page,
                "- `{}` —{}→ `{}`",
                md(name("source")),
                md(str_of(link, "rel_type")),
                md(name("target"))
            );
        }
    }
    page
}

/// Markdown wiki (an Obsidian vault, equally readable on GitHub, Forgejo or
/// in mdBook): `index.md` plus one note per multi-member community, linked
/// with plain Markdown links. Returns `(file name, contents)` pairs.
#[must_use]
pub fn wiki(graph: &Value, communities: &[Community], repo: &str) -> Vec<(String, String)> {
    let rank = ranks(communities);
    let nodes: BTreeMap<&str, &Value> = graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| (str_of(n, "id"), n))
        .collect();
    let mut crossing: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
    let mut internal: BTreeMap<usize, Vec<&Value>> = BTreeMap::new();
    for link in graph["links"].as_array().into_iter().flatten() {
        let (Some(&a), Some(&b)) = (
            rank.get(str_of(link, "source")),
            rank.get(str_of(link, "target")),
        ) else {
            continue;
        };
        if a == b {
            internal.entry(a).or_default().push(link);
        } else {
            *crossing.entry(a).or_default().entry(b).or_default() += 1;
            *crossing.entry(b).or_default().entry(a).or_default() += 1;
        }
    }
    let linked = |c: usize| communities[c].members.len() > 1;
    let mut index = format!(
        "# {}\n\nBuild `{}` · {} nodes · {} relationships · {} communities.\n\n| Community | Size |\n| --- | ---: |\n",
        md(repo),
        md(str_of(graph, "build_id")),
        nodes.len(),
        graph["links"].as_array().map_or(0, Vec::len),
        communities.len()
    );
    let mut files = Vec::new();
    for (c, community) in communities.iter().enumerate().filter(|(c, _)| linked(*c)) {
        let _ = writeln!(
            index,
            "| [{}]({}) | {} |",
            md(&community.label),
            note_name(c),
            community.members.len()
        );
        let page = community_page(
            c,
            communities,
            &nodes,
            crossing.get(&c),
            internal.get(&c).map_or(&[][..], Vec::as_slice),
        );
        files.push((note_name(c), page));
    }
    let singletons: Vec<&Community> = communities
        .iter()
        .filter(|c| c.members.len() == 1)
        .collect();
    if !singletons.is_empty() {
        let _ = writeln!(index, "\n## Unclustered ({})\n", singletons.len());
        for community in singletons.iter().take(500) {
            let node = nodes
                .get(community.members[0].as_str())
                .copied()
                .unwrap_or(&Value::Null);
            let _ = writeln!(
                index,
                "- `{}` ({})",
                md(str_of(node, "label")),
                md(str_of(node, "source_file"))
            );
        }
        if singletons.len() > 500 {
            let _ = writeln!(index, "- … {} more", singletons.len() - 500);
        }
    }
    files.insert(0, ("index.md".to_owned(), index));
    files
}
