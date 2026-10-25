//! Markdown graph report: Graphify's `GRAPH_REPORT.md`, rendered
//! deterministically from the analytics packet. Every listed item cites the
//! entity or relation id it rests on, next to the read that verifies it.

use std::fmt::Write as _;

use serde_json::Value;

/// Table-cell and inline-code safe text: no pipes, backticks or newlines.
fn md(text: &str) -> String {
    text.replace('|', "\\|")
        .replace('`', "'")
        .replace(['\n', '\r'], " ")
}

/// POSIX shell word for a copy-pasteable command.
fn sh(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_:./@+=-".contains(c))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// CLI and MCP forms of one read: `query <tool> <positional ids> --repo R`.
fn read(tool: &str, args: &Value, repo: &str) -> String {
    let positional: Vec<String> = ["rel", "id", "from", "to"]
        .iter()
        .filter_map(|key| args[key].as_str().map(sh))
        .collect();
    let mut mcp = args.clone();
    mcp["repo"] = Value::String(repo.to_owned());
    format!(
        "`chaosbox query {tool} {} --repo {}` · MCP `{tool}` `{}`",
        positional.join(" "),
        sh(repo),
        md(&mcp.to_string())
    )
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

/// Render the packet as Markdown for `repo`.
#[must_use]
pub fn markdown(packet: &Value, repo: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Graph report: {}\n", md(repo));
    let _ = writeln!(
        out,
        "Build `{}` (generation {}) · {} nodes · {} relationships · {} communities, {} \
         singletons (Leiden γ={}, modularity {}).\n",
        md(str_of(packet, "build_id")),
        packet["generation"],
        packet["nodes"],
        packet["edges"],
        packet["community_count"],
        packet["singletons"],
        packet["resolution"],
        packet["quality"],
    );
    let classes = &packet["edge_classes"];
    let _ = writeln!(
        out,
        "Evidence: {} extracted · {} inferred · {} ambiguous · {} unknown. \
         A missing relationship is not proof of absence.\n",
        classes["extracted"], classes["inferred"], classes["ambiguous"], classes["unknown"]
    );
    if let Some(refresh) = packet["refresh"].as_str() {
        let _ = writeln!(out, "> {}\n", md(refresh));
    }

    communities(&mut out, packet);
    god_nodes(&mut out, packet);
    surprises(&mut out, packet, repo);
    questions(&mut out, packet, repo);
    out
}

fn communities(out: &mut String, packet: &Value) {
    let _ = writeln!(out, "## Communities\n");
    let _ = writeln!(
        out,
        "| Label | Size | Cohesion | Hubs | Id |\n| --- | ---: | ---: | --- | --- |"
    );
    for group in packet["communities"].as_array().into_iter().flatten() {
        let hubs: Vec<String> = group["top"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|hub| format!("`{}`", md(str_of(hub, "label"))))
            .collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | `{}` |",
            md(str_of(group, "label")),
            group["size"],
            group["cohesion"],
            hubs.join(", "),
            md(str_of(group, "id"))
        );
    }
}

fn god_nodes(out: &mut String, packet: &Value) {
    let _ = writeln!(out, "\n## God nodes\n");
    let _ = writeln!(out, "| Symbol | Kind | File | Degree | Inferred | Id |\n| --- | --- | --- | ---: | ---: | --- |");
    for god in packet["god_nodes"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | `{}` |",
            md(str_of(god, "label")),
            md(str_of(god, "kind")),
            md(str_of(god, "source_file")),
            god["degree"],
            god["inferred_edges"],
            md(str_of(god, "id"))
        );
    }
}

fn surprises(out: &mut String, packet: &Value, repo: &str) {
    let _ = writeln!(out, "\n## Surprising connections\n");
    for (i, surprise) in packet["surprises"]
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
        .enumerate()
    {
        let why: Vec<String> = surprise["why"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(md)
            .collect();
        let _ = writeln!(
            out,
            "{}. `{}` —{}→ `{}` (score {}, {}): {}.  \n   {}",
            i + 1,
            md(str_of(surprise, "source_label")),
            md(str_of(surprise, "rel_type")),
            md(str_of(surprise, "target_label")),
            surprise["score"],
            md(str_of(surprise, "class")),
            why.join("; "),
            read(
                "evidence",
                &serde_json::json!({"rel": surprise["rel_id"]}),
                repo
            )
        );
    }
}

fn questions(out: &mut String, packet: &Value, repo: &str) {
    let _ = writeln!(out, "\n## Suggested questions\n");
    for (i, question) in packet["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let _ = writeln!(
            out,
            "{}. {}  \n   {}",
            i + 1,
            md(str_of(question, "text")),
            read(str_of(question, "tool"), &question["args"], repo)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{markdown, sh};
    use serde_json::json;

    #[test]
    fn report_cites_ids_with_runnable_reads_and_escapes_source_text() {
        let packet = json!({"build_id":"build:1","generation":2,"nodes":3,"edges":2,
            "community_count":1,"resolution":1.0,"quality":0.25,
            "edge_classes":{"extracted":1,"inferred":1,"ambiguous":0,"unknown":0},
            "communities":[{"id":"community:1","label":"src · a|b","size":3,"cohesion":0.667,
                "top":[{"label":"run`x"}]}],
            "god_nodes":[{"id":"ent:a","label":"run","kind":"definition","source_file":"src/a.rs",
                "degree":2,"inferred_edges":1}],
            "surprises":[{"rel_id":"rel:1","source_label":"run","target_label":"Doc","rel_type":"references",
                "score":4,"class":"inferred","why":["crosses code and documentation"]}],
            "questions":[{"text":"Why does `run` reach `Doc`?","tool":"path",
                "args":{"from":"ent:a","to":"ent:b c","repo":"r"}}]});
        let report = markdown(&packet, "r");
        assert!(report.contains("| src · a\\|b | 3 | 0.667 | `run'x` | `community:1` |"));
        assert!(report.contains("`chaosbox query evidence rel:1 --repo r`"));
        assert!(report.contains("`chaosbox query path ent:a 'ent:b c' --repo r`"));
        assert_eq!(report, markdown(&packet, "r"), "deterministic");
        assert_eq!(sh("it's"), r"'it'\''s'");
    }
}
