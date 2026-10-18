//! MCP surface unit tests, moved verbatim from the former inline `mod tests` block.

use super::mcp_server::{mcp_args, mcp_call_tool, mcp_tool_defs};

#[test]
fn tool_defs_are_read_only_with_schemas() {
    let defs = mcp_tool_defs();
    assert_eq!(defs.len(), 12);
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
fn neighbors_advertises_bounded_continuation() {
    let defs = mcp_tool_defs();
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
fn path_advertises_shared_bounds_and_rejects_unknown_fields() {
    let defs = mcp_tool_defs();
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
        ("max_hops", json!(18446744073709551615_u64)),
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
    let err = mcp_args("search", &serde_json::json!({})).unwrap_err();
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
async fn missing_repo_rejected_before_backend() {
    // No backend needed: the explicit-repo rule fires first.
    let resp = Box::pin(mcp_call_tool(
        &serde_json::json!(1),
        "search",
        &serde_json::json!({"arguments": {"query": "alpha"}}),
    ))
    .await;
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("repo"),
        "{resp}"
    );
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
    let defs = mcp_tool_defs();
    assert_eq!(defs.len(), 12);
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
