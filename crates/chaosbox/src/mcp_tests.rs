//! MCP surface unit tests, moved verbatim from the former inline `mod tests` block.

use super::mcp_server::{mcp_args, mcp_call_tool, mcp_tool_defs};

#[test]
fn tool_defs_are_read_only_with_schemas() {
    let defs = mcp_tool_defs();
    assert_eq!(defs.len(), 9);
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
