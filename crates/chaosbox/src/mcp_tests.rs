//! MCP surface unit tests, moved verbatim from the former inline `mod tests` block.

use super::mcp_server::{
    ChangedFile, changed_entities, is_test_path, prepare_neighborhood, prepare_path,
    review_payload, safe_join, serve_source, trim_payload, SourceInclude, SourceTarget,
    MCP_DESCRIPTION_LINE_MAX, MCP_NOT_ENABLED, char_budget, mcp_args, mcp_call_tool,
    mcp_call_tool_without_opt_in, mcp_instructions, mcp_tool_defs, mcp_tool_list,
};

/// Signals when a pending backend future is dropped.
struct Guard(tokio::sync::watch::Sender<bool>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

#[test]
fn tool_defs_are_read_only_with_schemas() {
    let defs = mcp_tool_defs(None);
    assert_eq!(defs.len(), 14);
    for d in &defs {
        assert_eq!(d["annotations"]["readOnlyHint"], true);
        assert!(d["inputSchema"]["properties"].is_object(), "{d}");
        assert!(d.get("_required").is_none(), "no internal fields leak: {d}");
        // Schema and runtime validation agree: every tool requires a
        // nonempty repo, so schema-valid calls cannot fail on identity.
        let required = d["inputSchema"]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            required.iter().any(|r| r == "repo"),
            "repo must be required: {d}"
        );
        assert_eq!(
            d["inputSchema"]["properties"]["repo"]["minLength"], 1,
            "repo must be nonempty: {d}"
        );
    }
}

#[test]
fn enabled_projects_default_the_repo_argument() {
    let defs = mcp_tool_defs(Some("demo"));
    assert_eq!(defs.len(), 14);
    for d in &defs {
        let schema = &d["inputSchema"];
        assert!(
            !schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("repo")),
            "repo is optional under an opt-in: {d}"
        );
        assert_eq!(schema["properties"]["repo"]["default"], "demo", "{d}");
        assert_eq!(schema["properties"]["repo"]["minLength"], 1, "{d}");
    }
    // An omitted repo resolves to the marker's repository; an explicit one wins.
    let args = mcp_args("status", &serde_json::json!({"arguments":{}}), Some("demo")).unwrap();
    assert_eq!(args["repo"], "demo");
    let args = mcp_args(
        "status",
        &serde_json::json!({"arguments":{"repo":"other"}}),
        Some("demo"),
    )
    .unwrap();
    assert_eq!(args["repo"], "other");
    let args = mcp_args("status", &serde_json::json!({"arguments":{}}), None).unwrap();
    assert!(args.get("repo").is_none());
}

#[tokio::test]
async fn unmarked_projects_advertise_no_graph_tools_and_answer_with_the_opt_in() {
    use serde_json::json;
    let defs = mcp_tool_list(None, false, false, false);
    assert!(defs.is_empty(), "no tools without an opt-in: {defs:?}");
    // Operator flags keep exactly their own tools.
    let defs = mcp_tool_list(None, true, false, true);
    let names: Vec<_> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "intelligence_context",
            "intelligence_evidence",
            "workspace_impact"
        ]
    );
    for name in ["search", "context", "lookup", "review"] {
        let response =
            mcp_call_tool_without_opt_in(&json!(1), name, &json!({"arguments":{}})).await;
        assert_eq!(response["error"]["code"], -32601, "{name}: {response}");
        assert_eq!(response["error"]["message"], MCP_NOT_ENABLED, "{name}");
    }
}

#[test]
fn tool_descriptions_open_with_when_to_use_and_stay_small() {
    // Baseline before the token-economy rewrite: 1439 characters.
    let budget = 1439;
    let defs = mcp_tool_defs(Some("demo"));
    let total: usize = defs
        .iter()
        .map(|tool| {
            tool["description"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .count()
        })
        .sum();
    assert!(total <= budget, "descriptions grew to {total} characters");
    for tool in &defs {
        let first = tool["description"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default();
        assert!(
            first.chars().count() <= MCP_DESCRIPTION_LINE_MAX,
            "{} first line is {} chars: {first}",
            tool["name"],
            first.chars().count()
        );
    }
}

#[test]
fn schemas_declare_every_default() {
    use serde_json::json;
    let defs = mcp_tool_defs(Some("demo"));
    let declared = [
        ("search", "limit", json!(20)),
        ("neighbors", "limit", json!(20)),
        ("neighbors", "direction", json!("both")),
        ("path", "max_hops", json!(4)),
        ("status", "detail", json!("summary")),
        ("lookup", "include", json!("source")),
        ("explain", "include", json!("source")),
        ("context", "depth", json!(3)),
        ("context", "max_nodes", json!(20)),
        ("context", "max_chars", json!(12000)),
        ("context", "include", json!("source")),
        ("stats", "limit", json!(10)),
        ("stats", "resolution", json!(1.0)),
        ("community", "limit", json!(100)),
        ("community", "resolution", json!(1.0)),
        ("report", "resolution", json!(1.0)),
        ("review", "base", json!("HEAD")),
    ];
    for (name, field, default) in declared {
        let tool = defs.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(
            tool["inputSchema"]["properties"][field]["default"], default,
            "{name}.{field} must declare its default"
        );
    }
    for tool in &defs {
        let schema = &tool["inputSchema"]["properties"]["max_tokens"];
        assert_eq!(schema["default"], 8000, "{}", tool["name"]);
        assert_eq!(schema["maximum"], 20000, "{}", tool["name"]);
    }
}

#[test]
fn repeated_budget_passes_keep_the_original_total() {
    use serde_json::json;
    let nodes: Vec<_> = (0..10)
        .map(|index| json!({"id": format!("ent:{index}")}))
        .collect();
    let mut payload = json!({"nodes": nodes});
    trim_payload("search", &mut payload, 120);
    assert_eq!(payload["truncated"]["nodes"]["total"], 10);
    let shown = payload["nodes"].as_array().unwrap().len();
    assert!(shown < 10, "the fixture must be trimmed");
    // A later pass only sees the trimmed list; the total stays the original.
    trim_payload("search", &mut payload, 80);
    assert_eq!(
        payload["truncated"]["nodes"]["total"], 10,
        "a second pass must not report the trimmed length as the total"
    );
    assert!(payload["nodes"].as_array().unwrap().len() < shown);
}

#[test]
fn large_results_are_trimmed_item_wise_under_the_budget() {
    use serde_json::json;
    let nodes: Vec<_> = (0..200)
        .map(|index| json!({"id": format!("ent:{index}"), "label": format!("symbol_{index}")}))
        .collect();
    let mut payload = json!({"nodes": nodes});
    assert_eq!(char_budget(&serde_json::Map::new()), 32_000);
    trim_payload("search", &mut payload, 4_000);
    assert!(serde_json::to_string(&payload).unwrap().len() <= 4_000);
    assert_eq!(payload["truncated"]["nodes"]["total"], 200);
    let shown = payload["nodes"].as_array().unwrap().len();
    assert_eq!(payload["truncated"]["nodes"]["shown"], shown);
    assert!(shown < 200, "the fixture must be trimmed");
    assert_eq!(payload["nodes"][0]["id"], "ent:0", "ranked order kept");
}

#[test]
fn source_serves_verified_lines_and_refuses_stale_or_escaping_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let text = "fn a() {}\nfn b() {\n    1\n}\n";
    std::fs::write(root.join("src/lib.rs"), text).unwrap();
    let mut digests = std::collections::BTreeMap::new();
    digests.insert(
        ("snap:one".to_owned(), "src/lib.rs".to_owned()),
        chaosbox_core::sha256_hex(&[text]),
    );
    let target = |file: &str| SourceTarget {
        file: file.to_owned(),
        snapshot: "snap:one".to_owned(),
        start: 2,
        end: 3,
    };
    let (entries, stale, served) = serve_source(
        root,
        &[target("src/lib.rs")],
        &digests,
        SourceInclude::Source,
        4096,
    );
    assert_eq!(entries[0]["file"], "src/lib.rs");
    assert_eq!(entries[0]["start_line"], 2);
    assert_eq!(entries[0]["end_line"], 3);
    assert_eq!(entries[0]["text"], "     2\tfn b() {\n     3\t    1\n");
    assert!(stale.is_empty());
    assert_eq!(served, ["src/lib.rs"]);

    // Indexed bytes no longer match the working tree: pointer only.
    std::fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
    let (entries, stale, served) = serve_source(
        root,
        &[target("src/lib.rs")],
        &digests,
        SourceInclude::Source,
        4096,
    );
    assert_eq!(entries[0]["stale"], true);
    assert!(entries[0].get("text").is_none());
    assert!(entries[0].get("end_line").is_some());
    assert_eq!(stale, ["src/lib.rs"]);
    assert!(served.is_empty());

    // Never outside the enabled root.
    assert!(safe_join(root, "../secret").is_none());
    assert!(safe_join(root, "/etc/passwd").is_none());
    assert!(safe_join(root, "src/../../secret").is_none());
    let (entries, _, _) = serve_source(
        root,
        &[target("../secret")],
        &digests,
        SourceInclude::Source,
        4096,
    );
    assert!(entries[0].get("text").is_none());

    // Restore the indexed bytes: signatures fall back to the span's first line.
    std::fs::write(root.join("src/lib.rs"), text).unwrap();
    let (entries, _, _) = serve_source(
        root,
        &[target("src/lib.rs")],
        &digests,
        SourceInclude::Signatures,
        4096,
    );
    assert_eq!(entries[0]["text"], "     2\tfn b() {\n");
    let (entries, _, _) = serve_source(
        root,
        &[target("src/lib.rs")],
        &digests,
        SourceInclude::None,
        4096,
    );
    assert!(entries[0].get("text").is_none());
}

#[test]
fn test_paths_are_recognized_for_the_review_safety_net() {
    for path in [
        "tests/it.rs",
        "src/test_helper.rs",
        "src/foo_test.rs",
        "web/app.test.ts",
        "spec/models/user.rb",
    ] {
        assert!(is_test_path(path), "{path}");
    }
    for path in ["src/lib.rs", "docs/agent.md", "crates/api/src/main.rs"] {
        assert!(!is_test_path(path), "{path}");
    }
}

#[test]
fn instructions_fit_the_mcp_prompt_budget() {
    let text = mcp_instructions("demo");
    assert!(text.len() <= 2000, "instructions are {} bytes", text.len());
    for tool in mcp_tool_defs(Some("demo")) {
        assert!(
            text.contains(tool["name"].as_str().unwrap()),
            "instructions omit {}",
            tool["name"]
        );
    }
}

#[test]
fn neighbors_advertises_bounded_continuation() {
    let defs = mcp_tool_defs(None);
    let neighbors = defs
        .iter()
        .find(|tool| tool["name"] == "neighbors")
        .unwrap();
    let schema = &neighbors["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    let props = &schema["properties"];
    assert_eq!(props["limit"]["default"], 20);
    assert_eq!(props["limit"]["minimum"], 1);
    assert_eq!(props["limit"]["maximum"], 200);
    assert_eq!(props["cursor"]["type"], "string");
    assert_eq!(
        props["direction"]["enum"],
        serde_json::json!(["both", "outgoing", "incoming"])
    );
    assert_eq!(props["rel"]["type"], "string");
    assert_eq!(props["rel_types"]["items"]["type"], "string");
}

#[test]
fn historical_tools_advertise_optional_exact_publication() {
    let defs = mcp_tool_defs(None);
    for name in ["lookup", "explain", "evidence", "neighbors"] {
        let tool = defs.iter().find(|tool| tool["name"] == name).unwrap();
        let schema = &tool["inputSchema"];
        let build = &schema["properties"]["build_id"];
        assert_eq!(build["type"], "string", "{name}");
        assert_eq!(build["minLength"], 1);
        assert_eq!(build["maxLength"], 256);
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("build_id")));
    }
}

#[tokio::test]
async fn historical_tools_validate_selection_before_credentials() {
    use serde_json::json;
    for name in ["lookup", "explain", "evidence", "neighbors"] {
        for build in [
            json!(null),
            json!(false),
            json!(1),
            json!([]),
            json!(""),
            json!(" "),
            json!("build:\ncontrol"),
            json!("é".repeat(129)),
        ] {
            let mut args = json!({"repo":"demo", "build_id":build});
            args[if name == "evidence" { "rel" } else { "id" }] = json!("fixture:id");
            let response = mcp_call_tool(&json!(1), name, &json!({"arguments":args})).await;
            assert_eq!(response["error"]["code"], -32602, "{name}: {response}");
            assert!(response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("build_id"));
        }
    }
}

#[tokio::test]
async fn historical_cursor_conflicts_fail_before_credentials() {
    use serde_json::json;
    let mut kinds = chaosbox::all_relation_types();
    kinds.sort();
    let cursor = json!({"contract":"chaosbox.repository-neighborhood", "version":1, "order":"rel_id_asc",
        "scope":{"repo":"demo", "id":"ent:hub", "direction":"both", "rel_types":kinds, "limit":20},
        "build_id":"build:old", "outgoing":{"after":"rel:first"}, "incoming":"done"});
    let response = mcp_call_tool(
        &json!(1),
        "neighbors",
        &json!({"arguments":{
            "repo":"demo", "id":"ent:hub", "build_id":"build:new", "cursor":cursor.to_string()
        }}),
    )
    .await;
    assert_eq!(response["error"]["code"], -32602, "{response}");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("conflicts"));
}

#[test]
fn path_advertises_shared_bounds_and_rejects_unknown_fields() {
    let defs = mcp_tool_defs(None);
    let path = defs.iter().find(|tool| tool["name"] == "path").unwrap();
    let schema = &path["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    for field in ["repo", "from", "to"] {
        assert_eq!(schema["properties"][field]["minLength"], 1);
        assert_eq!(schema["properties"][field]["maxLength"], 256);
    }
    let hops = &schema["properties"]["max_hops"];
    assert_eq!(hops["default"], 4);
    assert_eq!(hops["minimum"], 0);
    assert_eq!(hops["maximum"], 8);
}

#[tokio::test]
async fn path_rejects_malformed_arguments_before_credentials() {
    use serde_json::json;
    for (field, value) in [
        ("max_hops", json!(-1)),
        ("max_hops", json!(9)),
        ("max_hops", json!(1.5)),
        ("max_hops", json!("4")),
        ("max_hops", json!(true)),
        ("max_hops", json!(null)),
        ("max_hops", json!([])),
        ("max_hops", json!(u64::MAX)),
        ("repo", json!(" ")),
        ("repo", json!("x".repeat(257))),
        ("from", json!(" ")),
        ("from", json!("ent:\ncontrol")),
        ("from", json!("é".repeat(129))),
        ("to", json!("x".repeat(257))),
        ("unexpected", json!(true)),
    ] {
        let mut args = json!({"repo":"demo", "from":"ent:a", "to":"ent:b"});
        args[field] = value;
        let response = mcp_call_tool(&json!(1), "path", &json!({"arguments":args})).await;
        assert_eq!(response["error"]["code"], -32602, "{field}: {response}");
    }
}

#[tokio::test]
async fn neighbors_rejects_invalid_supplied_arguments_before_credentials() {
    use serde_json::json;
    for (field, value) in [
        ("limit", json!(0)),
        ("limit", json!(201)),
        ("limit", json!(-1)),
        ("limit", json!("3")),
        ("limit", json!(null)),
        ("direction", json!("sideways")),
        ("direction", json!(null)),
        ("cursor", json!(false)),
        ("cursor", json!(null)),
        ("cursor", json!("not json")),
        ("cursor", json!("x".repeat(20_000))),
        ("rel", json!(3)),
        ("rel", json!(null)),
        ("rel", json!("unknown")),
        ("rel_types", json!(null)),
        ("rel_types", json!("calls")),
        ("rel_types", json!(["calls", 3])),
        ("unexpected", json!(true)),
        ("repo", json!(" ")),
        ("id", json!("x".repeat(257))),
    ] {
        let mut args = json!({"repo":"demo", "id":"ent:hub"});
        args[field] = value;
        let response = mcp_call_tool(&json!(1), "neighbors", &json!({"arguments":args})).await;
        assert_eq!(response["error"]["code"], -32602, "{field}: {response}");
    }
    let response = mcp_call_tool(
        &json!(1),
        "neighbors",
        &json!({"arguments":{
            "repo":"demo", "id":"ent:hub", "rel":"calls", "rel_types":["calls"]
        }}),
    )
    .await;
    assert_eq!(response["error"]["code"], -32602, "{response}");
}

#[test]
fn missing_args_rejected_before_backend() {
    let err = mcp_args("search", &serde_json::json!({}), None).unwrap_err();
    assert_eq!(err["code"], -32602);
}

#[tokio::test]
async fn unknown_tools_rejected_without_backend() {
    // No backend needed: the closed tool set rejects first.
    for name in ["migrate", "evaluate", "db", "ingest", "annotate"] {
        let resp = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            name,
            &serde_json::json!({"name": name, "repo": "demo"}),
        ))
        .await;
        assert_eq!(resp["error"]["code"], -32601, "{name}: {resp}");
    }
}

#[tokio::test]
async fn missing_repo_is_an_argument_error_without_an_opt_in() {
    // No backend needed: the explicit-repo rule fires first when there is no
    // marker to default from.
    let resp = Box::pin(mcp_call_tool_without_opt_in(
        &serde_json::json!(1),
        "search",
        &serde_json::json!({"arguments": {"query": "alpha"}}),
    ))
    .await;
    assert_eq!(resp["error"]["code"], -32601, "{resp}");
    assert_eq!(resp["error"]["message"], MCP_NOT_ENABLED, "{resp}");
}

#[tokio::test]
async fn unbounded_queries_rejected_before_backend() {
    // No backend needed: service limits fire before connecting.
    let over_limit = Box::pin(mcp_call_tool(
        &serde_json::json!(1),
        "search",
        &serde_json::json!({"arguments": {"query": "alpha", "repo": "demo", "limit": 100_000}}),
    ))
    .await;
    assert_eq!(over_limit["error"]["code"], -32602, "{over_limit}");
    assert!(
        over_limit["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("limit"),
        "{over_limit}"
    );
    let over_hops = Box::pin(mcp_call_tool(
            &serde_json::json!(1),
            "path",
            &serde_json::json!({"arguments": {"from": "a", "to": "b", "repo": "demo", "max_hops": 1000}}),
        ))
        .await;
    assert_eq!(over_hops["error"]["code"], -32602, "{over_hops}");
    for (name, arguments) in [
        (
            "context",
            serde_json::json!({"repo":"demo","query":"entry","depth":100}),
        ),
        (
            "context",
            serde_json::json!({"repo":"demo","query":"entry","max_nodes":-1}),
        ),
        ("stats", serde_json::json!({"repo":"demo","limit":"all"})),
        ("stats", serde_json::json!({"repo":"demo","resolution":0})),
        ("report", serde_json::json!({"repo":"demo","resolution":11})),
        (
            "community",
            serde_json::json!({"repo":"demo","id":"c","resolution":"fine"}),
        ),
    ] {
        let response = mcp_call_tool(
            &serde_json::json!(1),
            name,
            &serde_json::json!({"arguments":arguments}),
        )
        .await;
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
}

#[tokio::test]
async fn calls_require_initialization_shape() {
    // Malformed (non-object) params fail arg validation, not the backend.
    let resp = Box::pin(mcp_call_tool(
        &serde_json::json!(1),
        "search",
        &serde_json::json!({"arguments": "not-an-object"}),
    ))
    .await;
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
}

#[test]
fn mcp_tool_set_loads_no_provider_credentials() {
    // Issue #8: the read-only MCP server never loads Jev credentials. The
    // tool definitions are static and credential-free, and this crate's
    // MCP path never calls `JevClient::api_key`: grep the source rather
    // than trusting the shape alone, so a future mutation tool cannot slip
    // past the closed set unnoticed.
    let defs = mcp_tool_defs(None);
    assert_eq!(defs.len(), 14);
    let names: Vec<_> = defs
        .iter()
        .map(|d| d["name"].as_str().unwrap_or_default())
        .collect();
    for forbidden in [
        "migrate", "ingest", "annotate", "evaluate", "decide", "run", "db",
    ] {
        assert!(
            !names.contains(&forbidden),
            "MCP must stay read-only: {names:?}"
        );
    }
    let src = include_str!("mcp_server.rs");
    assert!(
        !src.contains("api_key"),
        "mcp_server.rs must never load provider credentials"
    );
}

#[tokio::test]
async fn ordinary_frames_keep_partial_bytes_when_a_response_wins() {
    use super::mcp_server::{Frames, exchange, MCP_FRAME_BYTES};
    use tokio::io::{AsyncWriteExt, AsyncReadExt};
    let (mut sender, input) = tokio::io::duplex(MCP_FRAME_BYTES + 1);
    let mut frames = Frames::new(input);
    let (mut output, mut receiver) = tokio::io::duplex(1024);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sending = async {
        sender
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\",\"label\":\"")
            .await
            .unwrap();
        // Split a multi-byte UTF-8 character across the competing event.
        sender.write_all(&[0xc3]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        tx.send(()).unwrap();
    };
    let id = serde_json::json!(1);
    let serving = exchange(&mut frames, &mut output, &id, async {
        rx.await.unwrap();
        serde_json::json!({"id":1})
    });
    let (result, ()) = tokio::join!(serving, sending);
    assert!(result.unwrap());
    let mut response = vec![0; 9];
    receiver.read_exact(&mut response).await.unwrap();
    assert_eq!(response, b"{\"id\":1}\n");
    sender.write_all(&[0xa9, b'"', b'}', b'\n']).await.unwrap();
    let req: serde_json::Value =
        serde_json::from_slice(&frames.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(req["label"], "é");
    sender
        .write_all(&vec![b' '; MCP_FRAME_BYTES - 1])
        .await
        .unwrap();
    sender.write_all(b"\n").await.unwrap();
    assert_eq!(frames.next().await.unwrap().unwrap().len(), MCP_FRAME_BYTES);
    sender
        .write_all(&vec![b' '; MCP_FRAME_BYTES + 1])
        .await
        .unwrap();
    assert_eq!(
        frames.next().await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn ordinary_transport_drops_pending_reads_on_eof_oversize_and_closed_output() {
    use super::mcp_server::{Frames, exchange, MCP_FRAME_BYTES};
    use tokio::io::AsyncWriteExt;
    // A pending backend must be polled before any transport failure is injected.
    for case in ["eof", "oversize", "closed-output"] {
        let (mut sender, input) = tokio::io::duplex(MCP_FRAME_BYTES + 1);
        let (mut output, receiver) = tokio::io::duplex(1);
        let (started, start) = tokio::sync::oneshot::channel();
        let (dropped, mut drop_rx) = tokio::sync::watch::channel(false);
        let serving = async {
            exchange(
                &mut Frames::new(input),
                &mut output,
                &serde_json::json!(1),
                async {
                    let _guard = Guard(dropped);
                    started.send(()).unwrap();
                    std::future::pending().await
                },
            )
            .await
        };
        let failing = async {
            start.await.unwrap();
            if case == "eof" {
                drop(sender);
                drop(receiver);
            } else if case == "oversize" {
                sender
                    .write_all(&vec![b'x'; MCP_FRAME_BYTES + 1])
                    .await
                    .unwrap();
            } else {
                drop(receiver);
                sender
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n")
                    .await
                    .unwrap();
            }
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(serving, failing)
        })
        .await
        .unwrap();
        if case == "eof" {
            assert!(!result.unwrap());
        } else {
            assert!(result.is_err());
        }
        drop_rx.wait_for(|value| *value).await.unwrap();
    }
}

#[tokio::test]
async fn ordinary_response_overflow_is_an_error_and_keeps_the_rpc_id() {
    use super::mcp_server::{Frames, exchange, MCP_RESPONSE_BYTES};
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (_sender, input) = tokio::io::duplex(16);
    let (mut output, receiver) = tokio::io::duplex(1024);
    let mut frames = Frames::new(input);
    let id = serde_json::json!(7);
    let serving = exchange(&mut frames, &mut output, &id, async {
        serde_json::json!({"jsonrpc":"2.0", "id":7, "result":"x".repeat(MCP_RESPONSE_BYTES)})
    });
    let mut lines = BufReader::new(receiver).lines();
    let (result, line) = tokio::join!(serving, lines.next_line());
    assert!(result.unwrap());
    let response: serde_json::Value = serde_json::from_str(&line.unwrap().unwrap()).unwrap();
    assert_eq!(response["id"], 7);
    assert_eq!(response["error"]["code"], -32603);
}

#[tokio::test]
async fn ordinary_controls_drain_after_a_backpressured_final_response() {
    use super::mcp_server::{Frames, exchange};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (mut sender, input) = tokio::io::duplex(4096);
    let (mut output, receiver) = tokio::io::duplex(1);
    let id = serde_json::json!(1);
    let mut frames = Frames::new(input);
    let serving = exchange(&mut frames, &mut output, &id, async {
        serde_json::json!({"jsonrpc":"2.0", "id":1, "result":"x".repeat(1024)})
    });
    let client = async {
        sender.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":2}}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mut lines = BufReader::new(receiver).lines();
        for (index, expected) in [1, 2, 3, 1].into_iter().enumerate() {
            let response: serde_json::Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["id"], expected);
            if index == 0 {
                // A flushed final response releases its ID even while earlier
                // control responses remain backpressured.
                sender
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
                    .await
                    .unwrap();
            }
            if expected == 2 {
                assert_eq!(response["error"]["code"], -32800);
            } else {
                assert!(response.get("error").is_none(), "{response}");
            }
        }
        // Keep stdin open until exchange has drained every control response.
        sender
    };
    let (result, _sender) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(serving, client)
    })
    .await
    .unwrap();
    assert!(result.unwrap());
}

#[tokio::test]
async fn ordinary_slow_output_keeps_one_deadline_and_drops_the_active_read() {
    use super::mcp_server::{Frames, exchange};
    use tokio::io::AsyncWriteExt;
    let (mut sender, input) = tokio::io::duplex(4096);
    let (mut output, _unread) = tokio::io::duplex(1);
    let (started, start) = tokio::sync::oneshot::channel();
    let (dropped, mut drop_rx) = tokio::sync::watch::channel(false);
    let serving = async {
        exchange(
            &mut Frames::new(input),
            &mut output,
            &serde_json::json!(1),
            async {
                let _guard = Guard(dropped);
                started.send(()).unwrap();
                std::future::pending().await
            },
        )
        .await
    };
    let client = async {
        start.await.unwrap();
        sender
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        // Unrelated notifications neither cancel work nor restart the blocked
        // output deadline. Keep stdin open until the backend guard is released.
        loop {
            tokio::select! {
                _ = drop_rx.wait_for(|value| *value) => break,
                () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    sender.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":999}}\n").await.unwrap();
                }
            }
        }
    };
    let began = std::time::Instant::now();
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(12), async {
        tokio::join!(serving, client)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    assert!(began.elapsed() >= std::time::Duration::from_secs(9));
    assert!(*drop_rx.borrow());
}

/// Disposable fixture identity: temp repositories never touch operator config.
fn run_git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Chaosbox Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Chaosbox Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn span(file: &str, start: u32, end: u32) -> chaosbox_core::SourceSpan {
    chaosbox_core::SourceSpan {
        file: file.to_owned(),
        start_line: start,
        start_col: 1,
        end_line: end,
        end_col: 1,
        byte_start: 0,
        byte_end: 0,
    }
}

#[allow(clippy::too_many_lines)] // one fixture spells out the whole repository
#[tokio::test]
async fn review_maps_diff_hunks_to_callers_and_tests() {
    use chaosbox_core::{Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType};
    use chaosbox_store::MemoryReader;

    const REPO: &str = "demo";
    const SNAP: &str = "snap:fixture";
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "// header\nfn work() {\n    let x = 1;\n    let y = 2;\n    x + y\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/quiet.rs"), "fn quiet() {}\n").unwrap();
    std::fs::write(root.join("src/user.rs"), "fn user() { work(); }\n").unwrap();
    std::fs::write(root.join("tests/it.rs"), "#[test] fn t() { work(); }\n").unwrap();
    run_git(root, &["init", "-q"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "fixture"]);
    // One change inside the indexed definition, one outside every span.
    std::fs::write(
        root.join("src/lib.rs"),
        "// changed header\nfn work() {\n    let x = 1;\n    let y = 2;\n    x * y\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/quiet.rs"), "fn quiet() { }\n").unwrap();

    let work = Entity::new(
        EntityKind::Definition,
        REPO,
        SNAP,
        "src/lib.rs",
        "work",
        "work",
        span("src/lib.rs", 2, 6),
    );
    let quiet = Entity::new(
        EntityKind::Definition,
        REPO,
        SNAP,
        "src/quiet.rs",
        "quiet",
        "quiet",
        span("src/quiet.rs", 1, 1),
    );
    let caller = Entity::new(
        EntityKind::Definition,
        REPO,
        SNAP,
        "src/user.rs",
        "user",
        "user",
        span("src/user.rs", 1, 1),
    );
    let test = Entity::new(
        EntityKind::Definition,
        REPO,
        SNAP,
        "tests/it.rs",
        "t",
        "tests::t",
        span("tests/it.rs", 1, 1),
    );
    let mut build = GraphBuild::new(REPO, vec![SNAP.to_owned()], 1);
    for node in [&work, &quiet, &caller, &test] {
        build.add_node(node.clone()).unwrap();
    }
    build
        .add_edge(Relation::new(
            RelationType::Calls,
            &caller.id,
            &work.id,
            RelationScope::CrossFile,
            &build.id,
        ))
        .unwrap();
    build
        .add_edge(Relation::new(
            RelationType::References,
            &test.id,
            &work.id,
            RelationScope::CrossFile,
            &build.id,
        ))
        .unwrap();
    let mut memory = MemoryReader::new();
    memory.insert_build(build.clone());
    memory.set_active(REPO, &build.id);
    // The build indexed the base content of both changed files.
    memory.set_file_digest(
        SNAP,
        "src/lib.rs",
        &chaosbox_core::sha256_hex(&[
            "// header\nfn work() {\n    let x = 1;\n    let y = 2;\n    x + y\n}\n",
        ]),
    );
    memory.set_file_digest(
        SNAP,
        "src/quiet.rs",
        &chaosbox_core::sha256_hex(&["fn quiet() {}\n"]),
    );
    let reader = chaosbox::GraphReader::pinned(memory, REPO).await.unwrap();

    let review = review_payload(&reader, root, "HEAD", 4_000).await.unwrap();
    let entities = review["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 2, "{review}");
    assert_eq!(entities[0]["id"], work.id);
    assert_eq!(entities[0]["callers"], 2);
    assert_eq!(entities[0]["test_count"], 1);
    assert_eq!(entities[0]["tests"][0]["file"], "tests/it.rs");
    assert_eq!(entities[1]["id"], quiet.id);
    assert_eq!(entities[1]["callers"], 0);
    assert_eq!(review["unmapped"][0]["file"], "src/lib.rs");
    assert_eq!(review["unmapped"][0]["start_line"], 1);
    assert_eq!(review["changed_files"], 2);
    // The working tree moved past the indexed bytes, so text is not claimed fresh.
    for entry in review["source"].as_array().unwrap() {
        assert!(entry.get("text").is_none(), "{entry}");
        assert_eq!(entry["stale"], true, "{entry}");
    }
    assert!(serde_json::to_string(&review).unwrap().len() <= 4_000);

    // `base` is verified before any diff runs; a flag is never a revision.
    assert!(review_payload(&reader, root, "--help", 4_000)
        .await
        .is_err());
}

#[allow(clippy::too_many_lines)] // one fixture spells out the whole repository
#[tokio::test]
async fn review_maps_base_side_hunks_and_refuses_files_changed_since_indexing() {
    use chaosbox_core::{Entity, EntityKind, GraphBuild};
    use chaosbox_store::MemoryReader;

    const REPO: &str = "demo";
    const SNAP: &str = "snap:fixture";
    const BASE: &str =
        "fn first() {\n    let a = 1;\n    a\n}\nfn second() {\n    let b = 2;\n    b\n}\n";
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/shift.rs"), BASE).unwrap();
    std::fs::write(root.join("src/stale.rs"), "fn stale() {}\n").unwrap();
    run_git(root, &["init", "-q"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "fixture"]);
    // A line above every entity, plus one edit inside the second function.
    std::fs::write(
        root.join("src/shift.rs"),
        "// inserted\nfn first() {\n    let a = 1;\n    a\n}\nfn second() {\n    let b = 2;\n    b + 10\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/stale.rs"), "fn stale() { }\n").unwrap();

    let entity = |file: &str, name: &str, start: u32, end: u32| {
        Entity::new(
            EntityKind::Definition,
            REPO,
            SNAP,
            file,
            name,
            name,
            span(file, start, end),
        )
    };
    let first = entity("src/shift.rs", "first", 1, 4);
    let second = entity("src/shift.rs", "second", 5, 8);
    let stale = entity("src/stale.rs", "stale", 1, 1);
    let mut build = GraphBuild::new(REPO, vec![SNAP.to_owned()], 1);
    for node in [&first, &second, &stale] {
        build.add_node(node.clone()).unwrap();
    }
    let mut memory = MemoryReader::new();
    memory.insert_build(build.clone());
    memory.set_active(REPO, &build.id);
    memory.set_file_digest(SNAP, "src/shift.rs", &chaosbox_core::sha256_hex(&[BASE]));
    // Indexed at different bytes: the spans do not describe this file at base.
    memory.set_file_digest(
        SNAP,
        "src/stale.rs",
        &chaosbox_core::sha256_hex(&["fn aged() {}"]),
    );
    let reader = chaosbox::GraphReader::pinned(memory, REPO).await.unwrap();

    let review = review_payload(&reader, root, "HEAD", 4_000).await.unwrap();
    let entities = review["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 1, "{review}");
    assert_eq!(
        entities[0]["id"], second.id,
        "the insertion shifts nothing; only the edited function is affected"
    );
    let unmapped = review["unmapped"].as_array().unwrap();
    assert!(
        unmapped
            .iter()
            .any(|entry| entry["file"] == "src/stale.rs" && entry["reason"] == "stale"),
        "{review}"
    );
    assert!(
        !unmapped
            .iter()
            .any(|entry| entry["file"] == "src/stale.rs" && entry.get("start_line").is_some()),
        "a stale file is reported once, not hunk by hunk"
    );
}

#[test]
fn review_keeps_every_affected_sibling_entity() {
    use serde_json::json;
    let node = |id: &str, start: u32, end: u32| json!({"id": id, "source_file": "src/lib.rs", "span": {"start_line": start, "end_line": end}});
    let graph = json!({"nodes": [
        node("ent:module", 1, 20),
        node("ent:a", 1, 3),
        node("ent:b", 4, 12),
    ]});
    let files = [ChangedFile {
        base: Some("src/lib.rs".into()),
        file: "src/lib.rs".into(),
        hunks: vec![(3, 4)],
    }];
    let verified = std::collections::BTreeSet::from(["src/lib.rs".to_owned()]);
    let (entities, unmapped) = changed_entities(&graph, &files, &verified);
    let ids: Vec<_> = entities
        .iter()
        .map(|entity| entity["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["ent:a", "ent:b"],
        "both siblings survive; only the enclosing entity is dropped"
    );
    assert!(unmapped.is_empty());
}

#[test]
fn tool_level_arguments_do_not_reach_strict_query_structs() {
    use serde_json::json;
    for extra in [
        ("max_tokens", json!(500)),
        ("include", json!("source")),
        ("detail", json!("full")),
    ] {
        let mut path = json!({"repo":"demo","from":"ent:a","to":"ent:b"});
        path[extra.0] = extra.1.clone();
        let args = path.as_object().unwrap().clone();
        assert!(prepare_path(args).is_ok(), "path must ignore {}", extra.0);
        let mut neighbors = json!({"repo":"demo","id":"ent:hub","limit":20});
        neighbors[extra.0] = extra.1;
        let args = neighbors.as_object().unwrap().clone();
        assert!(
            prepare_neighborhood(args).is_ok(),
            "neighbors must ignore {}",
            extra.0
        );
    }
}
