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
/// Upper bound for caller-supplied path hop counts.
const MCP_MAX_HOPS: usize = 8;

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
        mcp_tool(
            "neighbors",
            "Incoming/outgoing neighborhoods with optional relation filter.",
            serde_json::json!({"id": {"type": "string"}, "rel": {"type": "string"}}),
            vec!["id"],
        ),
        mcp_tool(
            "path",
            "Bounded path between two entities (null when absent).",
            serde_json::json!({"from": {"type": "string"}, "to": {"type": "string"},
                "max_hops": {"type": "integer", "default": 4, "maximum": 8}}),
            vec!["from", "to"],
        ),
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
        "search" => &["query"],
        "lookup" | "neighbors" | "explain" => &["id"],
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
    if name == "path" {
        if let Some(h) = args.get("max_hops").and_then(serde_json::Value::as_u64) {
            if h > MCP_MAX_HOPS as u64 {
                return mcp_error(
                    id,
                    -32602,
                    format!("max_hops exceeds maximum {MCP_MAX_HOPS}"),
                    None,
                );
            }
        }
    }
    let reader = match Box::pin(AnyReader::connect(repo)).await {
        Ok(r) => r,
        Err(e) => {
            let report = LifecycleReport::error(&format!("mcp {name}"), &e.to_string());
            return mcp_error(
                id,
                -32603,
                e.to_string(),
                Some(serde_json::to_value(&report).unwrap()),
            );
        }
    };
    let payload: Result<serde_json::Value, String> =
        match name {
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
            "neighbors" => {
                let eid = args["id"].as_str().unwrap_or_default();
                let raw = args
                    .get("rel")
                    .and_then(|r| r.as_str())
                    .map(|r| vec![r.to_owned()]);
                let filter = match chaosbox::validate_rel_filter(raw) {
                    Ok(f) => f,
                    Err(e) => return mcp_error(id, -32602, e.to_string(), None),
                };
                reader.neighbors(eid, filter).await
                .map(|(out, inc)| serde_json::json!({"id": eid, "outgoing": out, "incoming": inc}))
                .map_err(|e| e.to_string())
            }
            "path" => {
                let from = args["from"].as_str().unwrap_or_default();
                let to = args["to"].as_str().unwrap_or_default();
                let hops = usize::try_from(
                    args.get("max_hops")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(4),
                )
                .expect("hop count fits in usize");
                reader
                    .path(from, to, hops)
                    .await
                    .map(|path| serde_json::json!({"from": from, "to": to, "path": path}))
                    .map_err(|e| e.to_string())
            }
            "evidence" => {
                let rel = args["rel"].as_str().unwrap_or_default();
                reader.evidence(rel).await.map_err(|e| e.to_string())
            }
            "status" => Ok(serde_json::json!({
                "repo": repo, "build_id": reader.build_id(), "generation": reader.generation(),
                "status": reader.status(), "snapshots": reader.snapshots(),
                "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
            })),
            "diff" => {
                let from = args["from_build"].as_str().unwrap_or_default();
                let to = args["to_build"].as_str().unwrap_or_default();
                reader.diff(repo, from, to).await.map_err(|e| e.to_string())
            }
            "export" => reader.export().await.map_err(|e| e.to_string()),
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
pub(super) async fn serve_mcp(intelligence: Option<chaosbox::intelligence::Bundle>) {
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
                    if intelligence.is_some() {
                        defs.push(mcp_tool("intelligence_context", "Small historical, source-backed knowledge packet; not instructions or current-state proof.", serde_json::json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}), vec!["query"]));
                        defs.push(mcp_tool(
                            "intelligence_evidence",
                            "Sources and typed decision receipts for a pinned intelligence item.",
                            serde_json::json!({"id":{"type":"string"}}),
                            vec!["id"],
                        ));
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
                    if name.starts_with("intelligence_") {
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
