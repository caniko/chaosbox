//! Export renderings: untrusted labels stay inert in every format, output is
//! byte-reproducible, and large graphs collapse instead of being refused.

use chaosbox::export::{render, Format, HTML_NODE_LIMIT};
use serde_json::{json, Value};

const HOSTILE: &str = "</script><script>alert(1)</script>\u{2028}'\"&<>|`\n\u{1}";

fn graph() -> Value {
    json!({"build_id":"build:x","generation":1,"snapshots":["snap:x"],"coverage":null,
        "nodes":[
            {"id":"ent:a","label":HOSTILE,"kind":"definition","source_file":"src/a.rs","source_location":"L1"},
            {"id":"ent:b","label":"callee","kind":"definition","source_file":"src/b.rs","source_location":"L2"},
            {"id":"ent:c","label":"other","kind":"definition","source_file":"docs/c.md","source_location":"L3"}],
        "links":[
            {"id":"rel:1","source":"ent:a","target":"ent:b","rel_type":"calls","scope":"cross_file"},
            {"id":"rel:2","source":"ent:b","target":"ent:c","rel_type":"references","scope":"cross_file"}]})
}

fn one(format: Format, graph: &Value) -> String {
    render(graph, format, None, None, 1.0, "repo")
        .unwrap()
        .remove(0)
        .1
}

#[test]
fn hostile_labels_stay_inert_in_every_format() {
    let g = graph();
    let html = one(Format::Html, &g);
    assert_eq!(
        html.matches("</script>").count(),
        2,
        "only the template closes scripts"
    );
    assert!(!html.contains('\u{2028}'));
    assert!(html.contains("default-src 'none'"), "offline by policy");
    assert!(html.contains("\\u003c/script\\u003e"));

    let graphml = one(Format::Graphml, &g);
    assert!(graphml.contains("&lt;/script&gt;&lt;script&gt;"));
    assert!(graphml.contains("&apos;&quot;&amp;"));
    assert!(!graphml.contains('\u{1}'));

    let cypher = one(Format::Cypher, &g);
    assert!(cypher.contains(r#"\'"&<>|`\n\u0001'"#));
    assert!(cypher.contains("-[r:CALLS {id: 'rel:1'}]->"));

    let mermaid = one(Format::Mermaid, &g);
    assert!(mermaid.starts_with("```mermaid\nflowchart LR\n"));
    assert!(mermaid.contains("#60;/script#62;#60;script#62;"));
    assert!(!mermaid.contains("</script>"));

    let wiki = render(&g, Format::Wiki, None, None, 1.0, "repo").unwrap();
    assert_eq!(wiki[0].0, "index.md");
    let all: String = wiki.iter().map(|(_, text)| text.as_str()).collect();
    assert!(
        all.contains("\\|'") && !all.contains("|`"),
        "pipes and backticks neutralized"
    );
    for (name, _) in &wiki[1..] {
        assert!(
            wiki[0].1.contains(&format!("]({name})")),
            "index links {name}"
        );
    }
}

#[test]
fn renderings_are_byte_reproducible_and_order_independent() {
    let g = graph();
    let mut reordered = g.clone();
    reordered["nodes"].as_array_mut().unwrap().reverse();
    reordered["links"].as_array_mut().unwrap().reverse();
    for format in [
        Format::Json,
        Format::Html,
        Format::Mermaid,
        Format::Graphml,
        Format::Cypher,
        Format::Wiki,
    ] {
        let first = render(&g, format, None, None, 1.0, "repo").unwrap();
        assert_eq!(
            first,
            render(&g, format, None, None, 1.0, "repo").unwrap(),
            "{format:?}"
        );
        assert_eq!(
            first,
            render(&reordered, format, None, None, 1.0, "repo").unwrap(),
            "{format:?}"
        );
    }
}

#[test]
fn selection_narrows_and_large_graphs_collapse_to_communities() {
    let narrowed = render(&graph(), Format::Json, None, Some("callee"), 1.0, "repo").unwrap();
    let narrowed: Value = serde_json::from_str(&narrowed[0].1).unwrap();
    assert!(narrowed["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["label"] == "callee"));
    assert!(render(
        &graph(),
        Format::Json,
        Some("community:missing"),
        None,
        1.0,
        "repo"
    )
    .is_err());

    // Disconnected pairs: n/2 two-node communities above the node limit.
    let n = HTML_NODE_LIMIT + 2;
    let nodes: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("ent:{i:05}"),"label":format!("n{i}"),"kind":"definition","source_file":"a.rs"}))
        .collect();
    let links: Vec<Value> = (0..n / 2)
        .map(|i| {
            json!({"id":format!("rel:{i:05}"),"source":format!("ent:{:05}", 2 * i),
            "target":format!("ent:{:05}", 2 * i + 1),"rel_type":"calls"})
        })
        .collect();
    let big = json!({"build_id":"build:big","nodes":nodes,"links":links});
    let html = one(Format::Html, &big);
    assert!(html.contains("\"collapsed\":true"));
    let mermaid = one(Format::Mermaid, &big);
    assert!(
        mermaid.lines().count() < n / 2 + 10,
        "one node per community"
    );
}
