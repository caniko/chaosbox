//! Read-only MCP over stdio: tool definitions, argument validation, dispatch, and the serve loop.

use super::{AnyReader, LifecycleReport, EXPORT_NODE_CAP, EXPORT_EDGE_CAP, all_relation_types};

// ---- Read-only MCP (JSON-RPC over stdio, full handshake) ----

/// MCP protocol version served here.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Older protocol versions still accepted from clients.
const MCP_PROTOCOL_FALLBACKS: &[&str] = &["2024-11-05", "2025-03-26"];
/// Tools per `tools/list` page.
const MCP_PAGE_SIZE: usize = 5;
/// Upper bound for caller-supplied search limits: paginate instead.
const MCP_SEARCH_LIMIT_MAX: i64 = 200;

pub(super) fn mcp_tool_defs() -> Vec<serde_json::Value> {
    vec![
        mcp_tool(
            "search",
            "Substring search over entity names (sorted, bounded).",
            serde_json::json!({"query": {"type": "string"}, "limit": {"type": "integer", "default": 20, "maximum": 200}}),
            vec!["query"],
        ),
        mcp_tool(
            "lookup",
            "Typed entity lookup by id.",
            serde_json::json!({"id": {"type": "string"}}),
            vec!["id"],
        ),
        {
            let mut tool = mcp_tool(
                "neighbors",
                "Page neighborhoods in relationship-ID order. Limit applies per direction. Follow next_cursor with the same arguments until complete; continuation pins the exact build. Use rel or rel_types, not both; an empty rel_types list matches nothing.",
                serde_json::json!({"id":{"type":"string","minLength":1,"maxLength":256},
                    "rel":{"type":"string"},"rel_types":{"type":"array","items":{"type":"string"}},
                    "limit":{"type":"integer","minimum":1,"maximum":200,"default":super::backend_cmd::NEIGHBORHOOD_DEFAULT_LIMIT},
                    "cursor":{"type":"string","maxLength":16384},
                    "direction":{"type":"string","enum":["both","outgoing","incoming"],"default":"both"}}),
                vec!["id"],
            );
            tool["inputSchema"]["additionalProperties"] = serde_json::json!(false);
            tool["inputSchema"]["not"] = serde_json::json!({"required":["rel","rel_types"]});
            tool
        },
        {
            let text = serde_json::json!({"type":"string","minLength":1,"maxLength":256});
            let mut tool = mcp_tool(
                "path",
                "Shortest undirected path within max_hops; null when absent, error on work/deadline exhaustion. Repository and endpoints must be nonblank, control-free and at most 256 UTF-8 bytes.",
                serde_json::json!({"from":text,"to":text,
                    "max_hops":{"type":"integer","minimum":0,
                        "default":chaosbox::PathQuery::DEFAULT_MAX_HOPS,
                        "maximum":chaosbox::PathQuery::MAX_HOPS}}),
                vec!["from", "to"],
            );
            tool["inputSchema"]["properties"]["repo"] = text;
            tool["inputSchema"]["additionalProperties"] = serde_json::json!(false);
            tool
        },
        mcp_tool(
            "evidence",
            "Claim evidence and source locations for a relationship.",
            serde_json::json!({"rel": {"type": "string"}}),
            vec!["rel"],
        ),
        mcp_tool(
            "status",
            "Active-build status, coverage, and generation.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "diff",
            "Node/edge id diff between two builds of one repo.",
            serde_json::json!({"from_build": {"type": "string"}, "to_build": {"type": "string"}}),
            vec!["from_build", "to_build"],
        ),
        mcp_tool(
            "export",
            "Deterministic export of the pinned active build.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "explain",
            "Source-backed entity explanation (no generated prose).",
            serde_json::json!({"id": {"type": "string"}}),
            vec!["id"],
        ),
        mcp_tool(
            "context",
            "Bounded lexical graph context with pinned source and relationship identities.",
            serde_json::json!({"query":{"type":"string","minLength":1,"maxLength":2048},"depth":{"type":"integer","minimum":1,"maximum":6},"max_nodes":{"type":"integer","minimum":1,"maximum":200},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}),
            vec!["query"],
        ),
        mcp_tool(
            "stats",
            "Deterministic counts, degree hubs and weak connectivity groups (not semantic clusters).",
            serde_json::json!({"limit":{"type":"integer","minimum":1,"maximum":200}}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "community",
            "Build-bound weak connectivity group, with explicit omitted members.",
            serde_json::json!({"id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}),
            vec!["id"],
        ),
    ]
}

// properties/required move into the schema json! below, which the
// pass-by-value lint cannot see through (macro boundary false positive).
#[allow(clippy::needless_pass_by_value)]
pub(super) fn mcp_tool(
    name: &str,
    description: &str,
    properties: serde_json::Value,
    required: Vec<&str>,
) -> serde_json::Value {
    let mut required = required;
    // Every tool requires an explicit repo: the server holds no default
    // (a silent `demo` fallback once sent agents to the wrong graph).
    // Declared here, not per tool, so schema and runtime validation agree.
    if !required.contains(&"repo") {
        required.push("repo");
    }
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.insert(
            "repo".to_owned(),
            serde_json::json!({"type": "string", "minLength": 1}),
        );
    }
    serde_json::json!({
        "name": name, "description": description,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": true},
    })
}

pub(super) fn mcp_text_result(
    id: &serde_json::Value,
    payload: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {"content": [{"type": "text", "text": serde_json::to_string(payload).unwrap_or_default()}]},
    })
}

// message moves into the error json! below (macro boundary false positive
// for the pass-by-value lint, same as mcp_tool above).
#[allow(clippy::needless_pass_by_value)]
pub(super) fn mcp_error(
    id: &serde_json::Value,
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut error = serde_json::json!({"code": code, "message": message});
    if let Some(d) = data {
        error["data"] = d;
    }
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// Validate tool arguments against required fields (types are checked per tool).
pub(super) fn mcp_args(
    name: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, serde_json::Value> {
    let args = params.get("arguments").unwrap_or(&serde_json::Value::Null);
    let map = args.as_object().cloned().unwrap_or_default();
    let required: &[&str] = match name {
        "search" | "context" => &["query"],
        "lookup" | "neighbors" | "explain" | "community" => &["id"],
        "path" => &["from", "to"],
        "evidence" => &["rel"],
        "diff" => &["from_build", "to_build"],
        _ => &[],
    };
    for key in required {
        if map
            .get(*key)
            .and_then(|v| v.as_str())
            .is_none_or(str::is_empty)
        {
            return Err(
                serde_json::json!({"code": -32602, "message": format!("missing required argument: {key}")}),
            );
        }
    }
    Ok(map)
}

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
pub(super) async fn mcp_call_tool(
    id: &serde_json::Value,
    name: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    let args = match mcp_args(name, params) {
        Ok(a) => a,
        Err(e) => {
            let code = e["code"].as_i64().unwrap_or(-32602);
            let msg = e["message"].as_str().unwrap_or("invalid params").to_owned();
            return mcp_error(id, code, msg, None);
        }
    };
    // Closed read-only tool set: reject unknown (write/mutation) tools before
    // touching the backend or credentials of any kind.
    if !matches!(
        name,
        "search"
            | "lookup"
            | "neighbors"
            | "path"
            | "evidence"
            | "status"
            | "diff"
            | "export"
            | "explain"
            | "context"
            | "stats"
            | "community"
    ) {
        return mcp_error(
            id,
            -32601,
            format!("read-only MCP: no such tool (rejected): {name}"),
            None,
        );
    }
    // Explicit repository: fail before touching the backend or credentials.
    let Some(repo) = args
        .get("repo")
        .and_then(|r| r.as_str())
        .filter(|r| !r.is_empty())
    else {
        return mcp_error(
            id,
            -32602,
            "missing required argument: repo".to_owned(),
            None,
        );
    };
    // Continuation selects its historical publication before any active pin.
    if name == "neighbors" {
        let query = match prepare_neighborhood(args) {
            Ok(query) => query,
            Err(error) => return mcp_error(id, -32602, error, None),
        };
        return match Box::pin(AnyReader::neighborhood_page(query)).await {
            Ok(page) => mcp_text_result(id, &serde_json::json!(page)),
            Err(error) => mcp_backend_error(id, name, &error),
        };
    }
    if name == "path" {
        let query = serde_json::from_value::<chaosbox::PathQuery>(serde_json::Value::Object(args))
            .map_err(|error| error.to_string())
            .and_then(|query| query.prepare().map_err(|error| error.to_string()));
        let query = match query {
            Ok(query) => query,
            Err(error) => return mcp_error(id, -32602, error, None),
        };
        return match Box::pin(AnyReader::path_query(query)).await {
            Ok(value) => mcp_text_result(id, &value),
            Err(error) => mcp_backend_error(id, name, &error),
        };
    }
    // Cheap service limits before any backend work: an unbounded query
    // would block the serial stdio loop for every later call.
    if name == "search" {
        if let Some(l) = args.get("limit").and_then(serde_json::Value::as_i64) {
            if l > MCP_SEARCH_LIMIT_MAX {
                return mcp_error(
                    id,
                    -32602,
                    format!("search limit exceeds maximum {MCP_SEARCH_LIMIT_MAX}"),
                    None,
                );
            }
        }
    }
    let bounds: &[(&str, u64, u64)] = match name {
        "context" => &[
            ("depth", 1, 6),
            ("max_nodes", 1, 200),
            ("max_chars", 256, 32_000),
        ],
        "stats" | "community" => &[("limit", 1, 200)],
        _ => &[],
    };
    for (key, minimum, maximum) in bounds {
        if args.get(*key).is_some_and(|value| {
            !value
                .as_u64()
                .is_some_and(|n| (*minimum..=*maximum).contains(&n))
        }) {
            return mcp_error(
                id,
                -32602,
                format!("{key} must be an integer in {minimum}..{maximum}"),
                None,
            );
        }
    }
    if name == "context"
        && args["query"]
            .as_str()
            .is_none_or(|query| query.trim().is_empty() || query.len() > 2048)
    {
        return mcp_error(
            id,
            -32602,
            "context query must be 1..2048 bytes".into(),
            None,
        );
    }
    let reader = match Box::pin(AnyReader::connect(repo)).await {
        Ok(r) => r,
        Err(e) => return mcp_backend_error(id, name, &e),
    };
    let payload: Result<serde_json::Value, String> = match name {
        "search" => {
            let q = args["query"].as_str().unwrap_or_default();
            let limit = args
                .get("limit")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(20);
            reader
                .search(q, limit)
                .await
                .map(|rows| serde_json::to_value(&rows).unwrap())
                .map_err(|e| e.to_string())
        }
        "lookup" => {
            let eid = args["id"].as_str().unwrap_or_default();
            reader
                .lookup(eid)
                .await
                .map(|row| serde_json::to_value(&row).unwrap())
                .map_err(|e| e.to_string())
        }
        "evidence" => {
            let rel = args["rel"].as_str().unwrap_or_default();
            reader.evidence(rel).await.map_err(|e| e.to_string())
        }
        "status" => Ok(serde_json::json!({
            "repo": repo, "build_id": reader.build_id(), "generation": reader.generation(),
            "status": reader.status(), "snapshots": reader.snapshots(),
            "coverage": reader.coverage.as_ref().map(chaosbox_core::coverage::BuildCoverage::report),
            "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
        })),
        "diff" => {
            let from = args["from_build"].as_str().unwrap_or_default();
            let to = args["to_build"].as_str().unwrap_or_default();
            reader.diff(repo, from, to).await.map_err(|e| e.to_string())
        }
        "export" => reader.export().await.map_err(|e| e.to_string()),
        "context" => reader
            .context(
                args["query"].as_str().unwrap_or_default(),
                integer(args.get("depth"), 3),
                integer(args.get("max_nodes"), 20),
                integer(args.get("max_chars"), 12000),
            )
            .await
            .map_err(|e| e.to_string()),
        "stats" => reader
            .stats(integer(args.get("limit"), 10))
            .await
            .map_err(|e| e.to_string()),
        "community" => reader
            .community(
                args["id"].as_str().unwrap_or_default(),
                integer(args.get("limit"), 100),
            )
            .await
            .map_err(|e| e.to_string()),
        "explain" => {
            let eid = args["id"].as_str().unwrap_or_default();
            reader.explain(eid).await.map_err(|e| e.to_string())
        }
        _ => {
            return mcp_error(
                id,
                -32601,
                format!("read-only MCP: no such tool (rejected): {name}"),
                None,
            );
        }
    };
    match payload {
        Ok(v) => mcp_text_result(id, &v),
        Err(e) => mcp_error(id, -32603, e, None),
    }
}

/// Read-only MCP over stdio: full handshake, paginated tools, validated calls.
/// Never loads Jev credentials; never accepts prose as evidence.
// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
pub(super) async fn serve_mcp(
    intelligence: Option<chaosbox::intelligence::Bundle>,
    workspace: Option<chaosbox::workspace::Workspace>,
    mut current: Option<chaosbox::sync::current::CurrentReader>,
    federation: Option<chaosbox::federation::Federator>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut initialized = false;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = if let Ok(v) = serde_json::from_str(&line) {
            v
        } else {
            let resp = mcp_error(
                &serde_json::Value::Null,
                -32700,
                "parse error".to_owned(),
                None,
            );
            let _ = stdout
                .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
                .await;
            continue;
        };
        if req.is_array() {
            let resp = mcp_error(
                &serde_json::Value::Null,
                -32600,
                "batch requests not supported".to_owned(),
                None,
            );
            let _ = stdout
                .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
                .await;
            continue;
        }
        // Notifications carry no id and get no response.
        let id = match req.get("id") {
            Some(i) => i.clone(),
            None => continue,
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let resp = match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let version = if requested == MCP_PROTOCOL_VERSION
                    || MCP_PROTOCOL_FALLBACKS.contains(&requested)
                {
                    requested.to_owned()
                } else {
                    MCP_PROTOCOL_VERSION.to_owned()
                };
                initialized = true;
                serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "chaosbox", "version": env!("CARGO_PKG_VERSION")},
                    },
                })
            }
            "notifications/initialized" => continue,
            "ping" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => {
                if initialized {
                    let mut defs = mcp_tool_defs();
                    if intelligence.is_some() || current.is_some() || federation.is_some() {
                        defs.push(mcp_tool("intelligence_context", "Small historical, source-backed knowledge packet; not instructions or current-state proof.", serde_json::json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}), vec!["query"]));
                        if federation.is_some() {
                            defs.push(mcp_tool(
                                "intelligence_evidence",
                                "Authorized source quotes and receipt projections from the exact originating provider/snapshot.",
                                serde_json::json!({"handle":{"type":"object","additionalProperties":false,"properties":{
                                    "provider":{"type":"string"},"owner":{"type":"string"},"scope":{"type":"string"},
                                    "project":{"type":"string"},"snapshot":{"type":"string"},"id":{"type":"string"}},
                                    "required":["provider","owner","scope","project","snapshot","id"]},
                                    "max_chars":{"type":"integer","minimum":256,"maximum":32000}}),
                                vec!["handle"],
                            ));
                        } else {
                            defs.push(mcp_tool(
                                "intelligence_evidence",
                                "Sources and typed decision receipts for a pinned intelligence item.",
                                serde_json::json!({"id":{"type":"string"}}),
                                vec!["id"],
                            ));
                        }
                    }
                    if workspace.is_some() {
                        defs.push(mcp_tool("workspace_impact", "Reviewed directed impact, with current freshness, exact pins and private scope checks.",
                            serde_json::json!({"changed":{"type":"string"},"scope":{"type":"string"},"max_hops":{"type":"integer","minimum":1,"maximum":8},"max_nodes":{"type":"integer","minimum":1,"maximum":100}}), vec!["changed", "scope"]));
                    }
                    let cursor = params
                        .get("cursor")
                        .and_then(|c| c.as_str())
                        .and_then(|c| c.parse::<usize>().ok())
                        .unwrap_or(0);
                    let page: Vec<_> = defs.into_iter().skip(cursor).take(MCP_PAGE_SIZE).collect();
                    let next = if page.len() == MCP_PAGE_SIZE {
                        Some((cursor + MCP_PAGE_SIZE).to_string())
                    } else {
                        None
                    };
                    let mut result = serde_json::json!({"tools": page});
                    if let Some(n) = next {
                        result["nextCursor"] = serde_json::Value::String(n);
                    }
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            "tools/call" => {
                if initialized {
                    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name == "workspace_impact" {
                        match workspace.as_ref() {
                            Some(workspace) => {
                                match workspace_call(workspace, &params["arguments"]) {
                                    Ok(value) => mcp_text_result(&id, &value),
                                    Err(error) => mcp_error(&id, -32602, error, None),
                                }
                            }
                            None => mcp_error(
                                &id,
                                -32601,
                                "no workspace artifact configured".into(),
                                None,
                            ),
                        }
                    } else if name.starts_with("intelligence_") {
                        if let Some(reader) = federation.as_ref() {
                            match reader.query(name, &params["arguments"]).await {
                                Ok(value) => mcp_text_result(&id, &value),
                                Err(error) => mcp_error(&id, -32602, error.to_string(), None),
                            }
                        } else if let Some(reader) = current.as_mut() {
                            match reader.query(name, &params["arguments"]).await {
                                Ok(value) => mcp_text_result(&id, &value),
                                Err(error) => mcp_error(&id, -32602, error, None),
                            }
                        } else {
                            match intelligence.as_ref() {
                                Some(bundle) => match chaosbox::intelligence::mcp_query(
                                    bundle,
                                    name,
                                    &params["arguments"],
                                ) {
                                    Ok(value) => mcp_text_result(&id, &value),
                                    Err(error) => mcp_error(&id, -32602, error, None),
                                },
                                None => mcp_error(
                                    &id,
                                    -32601,
                                    "no intelligence bundle configured".into(),
                                    None,
                                ),
                            }
                        }
                    } else {
                        Box::pin(mcp_call_tool(&id, name, &params)).await
                    }
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            _ => mcp_error(
                &id,
                -32601,
                format!("unknown method (rejected): {method}"),
                None,
            ),
        };
        let _ = stdout
            .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
            .await;
    }
    let _ = all_relation_types;
}

fn workspace_call(
    workspace: &chaosbox::workspace::Workspace,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let scope = args["scope"].as_str().ok_or("missing scope")?;
    let repo = args["repo"].as_str().ok_or("missing repo")?;
    let changed = args["changed"].as_str().ok_or("missing changed endpoint")?;
    if workspace
        .endpoints
        .get(changed)
        .is_none_or(|endpoint| endpoint.member != repo)
    {
        return Err("changed endpoint is outside requested repository".into());
    }
    for (key, maximum) in [("max_hops", 8), ("max_nodes", 100)] {
        if args
            .get(key)
            .is_some_and(|value| !value.as_u64().is_some_and(|n| (1..=maximum).contains(&n)))
        {
            return Err(format!("{key} must be an integer in 1..{maximum}"));
        }
    }
    workspace
        .impact(
            scope,
            changed,
            integer(args.get("max_hops"), 4),
            integer(args.get("max_nodes"), 20),
        )
        .map_err(|e| e.to_string())
}

fn prepare_neighborhood(
    mut args: serde_json::Map<String, serde_json::Value>,
) -> Result<chaosbox::PreparedNeighborhood, String> {
    if let Some(rel) = args.remove("rel") {
        if args.contains_key("rel_types") {
            return Err("rel and rel_types are mutually exclusive".into());
        }
        let rel = rel.as_str().ok_or("rel must be a string")?;
        args.insert("rel_types".into(), serde_json::json!([rel]));
    }
    for (key, valid) in [
        (
            "cursor",
            args.get("cursor").is_none_or(serde_json::Value::is_string),
        ),
        (
            "rel_types",
            args.get("rel_types")
                .is_none_or(serde_json::Value::is_array),
        ),
    ] {
        if !valid {
            return Err(format!("{key} has an invalid type"));
        }
    }
    args.entry("limit").or_insert(serde_json::json!(
        super::backend_cmd::NEIGHBORHOOD_DEFAULT_LIMIT
    ));
    let query: chaosbox::NeighborhoodQuery =
        serde_json::from_value(serde_json::Value::Object(args))
            .map_err(|error| error.to_string())?;
    query.prepare().map_err(|error| error.to_string())
}

fn mcp_backend_error(
    id: &serde_json::Value,
    name: &str,
    error: &impl std::fmt::Display,
) -> serde_json::Value {
    let report = LifecycleReport::error(&format!("mcp {name}"), &error.to_string());
    mcp_error(
        id,
        -32603,
        error.to_string(),
        Some(serde_json::json!(report)),
    )
}

fn integer(value: Option<&serde_json::Value>, default: usize) -> usize {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(default)
}
