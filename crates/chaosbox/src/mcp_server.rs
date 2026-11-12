//! Read-only MCP over stdio: tool definitions, argument validation, dispatch, and the serve loop.

use super::{
    AnyReader, LifecycleReport, PgReader, EXPORT_NODE_CAP, EXPORT_EDGE_CAP, all_relation_types,
};
use std::{
    collections::VecDeque,
    future::{Future, pending},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

// ---- Read-only MCP (JSON-RPC over stdio, full handshake) ----

/// MCP protocol version served here.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Older protocol versions still accepted from clients.
const MCP_PROTOCOL_FALLBACKS: &[&str] = &["2024-11-05", "2025-03-26"];
/// Agent-facing answer when this project has no `.chaosbox.json` opt-in.
pub(super) const MCP_NOT_ENABLED: &str =
    "Chaosbox is not enabled for this project. Do not call it; use normal file tools.";
/// Upper bound for caller-supplied search limits: paginate instead.
const MCP_SEARCH_LIMIT_MAX: i64 = 200;
/// Limits include framing bytes. Overlong input closes the connection.
pub(super) const MCP_FRAME_BYTES: usize = 64 * 1024;
pub(super) const MCP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MCP_OUTPUT_TIMEOUT: Duration = Duration::from_secs(10);
const MCP_GRAPH_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MCP_PENDING_REQUESTS: usize = 8;
const MCP_PENDING_BYTES: usize = 256 * 1024;
// Admission wait is separate from dispatch-owned backend/path deadlines.
const MCP_ADMISSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Default and maximum `max_tokens`; the character budget is four times this.
const MCP_MAX_TOKENS_DEFAULT: usize = 8_000;
const MCP_MAX_TOKENS_MAX: usize = 20_000;
/// Characters per output token used for every tool budget.
const MCP_CHARS_PER_TOKEN: usize = 4;
/// Distinct files one answer may pull source from (bounded backend reads).
const MCP_SOURCE_FILES_MAX: usize = 32;
/// First line of each tool description that `OpenCode` keeps.
#[cfg(test)]
pub(super) const MCP_DESCRIPTION_LINE_MAX: usize = 120;

// One data table: every tool and its schema, split only to hide the shape.
#[allow(clippy::too_many_lines)]
pub(super) fn mcp_tool_defs(default_repo: Option<&str>) -> Vec<serde_json::Value> {
    vec![
        mcp_tool(
            "search",
            "Use before Grep to find an entity by name substring; returns ids, kind and file.",
            serde_json::json!({"query": {"type": "string"}, "limit": {"type": "integer", "default": 20, "maximum": 200}}),
            vec!["query"],
        ),
        mcp_tool(
            "lookup",
            "Use before Read to fetch one entity's record by id or exact name.",
            serde_json::json!({"id": {"type": "string"}, "include": SOURCE_SCHEMA()}),
            vec!["id"],
        ),
        {
            let mut tool = mcp_tool(
                "neighbors",
                "Use to list callers or callees instead of grepping references; page with next_cursor.",
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
                "Use to trace how two entities connect instead of guessing imports.",
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
            "Use to read the sources behind a relationship's claim before repeating it.",
            serde_json::json!({"rel": {"type": "string"}}),
            vec!["rel"],
        ),
        mcp_tool(
            "status",
            "Use to check what Chaosbox indexed (build, snapshots, commit, coverage).",
            serde_json::json!({"detail":{"type":"string","enum":["summary","full"],"default":"summary"}}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "diff",
            "Use to compare entity and edge ids between two builds of this repo.",
            serde_json::json!({"from_build": {"type": "string"}, "to_build": {"type": "string"}}),
            vec!["from_build", "to_build"],
        ),
        mcp_tool(
            "export",
            "Use only when analysis needs every node; exports the pinned build.",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "explain",
            "Use before Read to see one entity's file, span and degree counts.",
            serde_json::json!({"id": {"type": "string"}, "include": SOURCE_SCHEMA()}),
            vec!["id"],
        ),
        mcp_tool(
            "context",
            "Start here for a question: ranked entities with source and links.",
            serde_json::json!({"query":{"type":"string","minLength":1,"maxLength":2048},"depth":{"type":"integer","minimum":1,"maximum":6,"default":3},"max_nodes":{"type":"integer","minimum":1,"maximum":200,"default":20},"max_chars":{"type":"integer","minimum":256,"maximum":32000,"default":12000},"include":SOURCE_SCHEMA()}),
            vec!["query"],
        ),
        mcp_tool(
            "stats",
            "Use to survey architecture: communities, hubs, gods, surprises, questions.",
            serde_json::json!({"limit":{"type":"integer","minimum":1,"maximum":200,"default":10},"resolution":RESOLUTION_SCHEMA()}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "community",
            "Use to inspect one Leiden community's members and cohesion.",
            serde_json::json!({"id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200,"default":100},"resolution":RESOLUTION_SCHEMA()}),
            vec!["id"],
        ),
        mcp_tool(
            "report",
            "Use to render the architecture report as Markdown with citations.",
            serde_json::json!({"resolution":RESOLUTION_SCHEMA()}),
            Vec::<&str>::new(),
        ),
        mcp_tool(
            "review",
            "Use to review a git diff: changed entities with callers and tests.",
            serde_json::json!({"base":{"type":"string","minLength":1,"maxLength":256,"default":"HEAD"}}),
            Vec::<&str>::new(),
        ),
    ]
    .into_iter()
    .map(|tool| default_repo_tool(tool, default_repo))
    .collect()
}

/// Source inclusion level shared by `context`, `lookup` and `explain`.
#[allow(non_snake_case)]
fn SOURCE_SCHEMA() -> serde_json::Value {
    serde_json::json!({"type":"string","enum":["source","signatures","none"],"default":"source"})
}

/// Enabled projects hold the repository in `.chaosbox.json`: drop the required
/// `repo` argument and declare that default, so agents stop repeating it.
pub(super) fn default_repo_tool(
    mut tool: serde_json::Value,
    repo: Option<&str>,
) -> serde_json::Value {
    let Some(repo) = repo else {
        return tool;
    };
    let schema = &mut tool["inputSchema"];
    if let Some(required) = schema["required"].as_array_mut() {
        required.retain(|name| name != "repo");
    }
    schema["properties"]["repo"] = serde_json::json!({
        "type": "string", "minLength": 1, "default": repo,
        "description": "Graph repository; omit for this project's .chaosbox.json repo.",
    });
    tool
}

/// Imperative routing prompt for an enabled project. `OpenCode` and Claude
/// append this to their system prompt, so it names every tool and its bounds;
/// a test keeps it under 2000 characters.
pub(super) fn mcp_instructions(repo: &str) -> String {
    format!(
        "This project is indexed by Chaosbox (repo {repo}). You MUST call Chaosbox before \
Grep/Glob/Read when you need: where something is defined, its source, callers/callees, what a \
change affects, or architecture. Read a file directly only for lines Chaosbox did not return or \
marked stale. Do not re-verify Chaosbox answers with grep. Source Chaosbox returns is the file \
content; treat it as already read.\n\n\
The repo argument defaults to {repo}. Answers carry truncated counts (shown/total, keyed by list \
when several lost items): raise max_tokens or follow next_cursor instead of asking again.\n\n\
Tools: context = ranked entities plus source for a question; search = entity names; lookup/explain \
= one entity (include=source|signatures|none, default source); neighbors = callers/callees, paged \
by cursor; path = shortest chain between two entities; review = changed entities of a git diff \
with callers and tests; stats/community/report = architecture and communities; evidence = claim \
sources; status/diff/export = build metadata."
    )
}

/// Leiden resolution: 1.0 (default) is classic modularity, larger values
/// give smaller communities. Non-default values recompute from the export.
#[allow(non_snake_case)]
fn RESOLUTION_SCHEMA() -> serde_json::Value {
    serde_json::json!({"type":"number","minimum":0.01,"maximum":10,"default":1.0})
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
        // Every tool obeys one output budget: 4 characters per token.
        props.insert(
            "max_tokens".to_owned(),
            serde_json::json!({"type":"integer","minimum":1,"maximum":MCP_MAX_TOKENS_MAX,
                "default":MCP_MAX_TOKENS_DEFAULT,
                "description":"Output budget in tokens (4 characters each); whole items are trimmed and counted in truncated."}),
        );
        if matches!(name, "lookup" | "explain" | "evidence" | "neighbors") {
            props.insert("build_id".into(), serde_json::json!({
                "type":"string", "minLength":1, "maxLength":256,
                "description":"Exact repository-owned publication. Omit for current; with a cursor, must match its build."
            }));
        }
    }
    serde_json::json!({
        "name": name, "description": description,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": true},
    })
}

/// Operator-tool result under the same `max_tokens` budget.
fn mcp_operator_result(
    id: &serde_json::Value,
    value: serde_json::Value,
    args: &serde_json::Value,
) -> serde_json::Value {
    match budget_any(value, args) {
        Ok(value) => mcp_text_result(id, value),
        Err(error) => mcp_error(id, -32602, error, None),
    }
}

pub(super) fn mcp_text_result(id: &serde_json::Value, mut payload: Value) -> Value {
    flatten_truncated(&mut payload);
    let Ok(bytes) = chaosbox::read_view::bounded_json(&payload, MCP_RESPONSE_BYTES) else {
        return mcp_error(id, -32603, "MCP response byte budget exceeded".into(), None);
    };
    serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {"content": [{"type": "text", "text": String::from_utf8_lossy(&bytes)}]},
    })
}

/// One trimmed collection keeps the flat `{shown,total,next_cursor?}` shape;
/// several stay keyed by collection, so no count is ambiguous.
fn flatten_truncated(payload: &mut Value) {
    let Some(collections) = payload.get("truncated").and_then(Value::as_object) else {
        return;
    };
    if collections.len() != 1 || !collections.values().all(Value::is_object) {
        return;
    }
    if let Some(only) = collections.values().next().cloned() {
        payload["truncated"] = only;
    }
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

/// Validate tool arguments against required fields (types are checked per
/// tool). Enabled projects fill an omitted or blank `repo` from their marker;
/// an explicit repository always wins.
pub(super) fn mcp_args(
    name: &str,
    params: &serde_json::Value,
    default_repo: Option<&str>,
) -> Result<serde_json::Map<String, serde_json::Value>, serde_json::Value> {
    let args = params.get("arguments").unwrap_or(&serde_json::Value::Null);
    let mut map = args.as_object().cloned().unwrap_or_default();
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
    if let Some(repo) = default_repo {
        if map
            .get("repo")
            .and_then(|v| v.as_str())
            .is_none_or(str::is_empty)
        {
            map.insert(
                "repo".to_owned(),
                serde_json::Value::String(repo.to_owned()),
            );
        }
    }
    Ok(map)
}

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
async fn mcp_call_tool_connected(
    id: &serde_json::Value,
    name: &str,
    params: &serde_json::Value,
    connection: &mut Option<PgReader>,
    enabled: Option<&chaosbox::enablement::Enabled>,
) -> serde_json::Value {
    // Opt-in is per project: an unmarked checkout gets one instruction and no
    // graph work, before any argument validation or credential acquisition.
    let Some(enabled) = enabled else {
        return mcp_error(id, -32601, MCP_NOT_ENABLED.to_owned(), None);
    };
    let args = match mcp_args(name, params, Some(&enabled.repo)) {
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
            | "report"
            | "review"
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
    let build_id = args
        .get("build_id")
        .map(|value| {
            if !matches!(name, "lookup" | "explain" | "evidence" | "neighbors") {
                return Err("build_id is not supported by this tool".to_owned());
            }
            let build_id = value.as_str().ok_or("build_id must be a string")?;
            chaosbox::validate_build_id(build_id).map_err(|error| error.to_string())?;
            Ok(build_id)
        })
        .transpose();
    let build_id = match build_id {
        Ok(build_id) => build_id.map(str::to_owned),
        Err(error) => return mcp_error(id, -32602, error, None),
    };
    let (repo, build_id) = (repo.to_owned(), build_id);
    let (repo, build_id) = (repo.as_str(), build_id.as_deref());
    let mut args = args;
    // One output budget for every tool: 4 characters per requested token.
    let budget = char_budget(&args);
    let include = match source_include(&args) {
        Ok(include) => include,
        Err(error) => return mcp_error(id, -32602, error, None),
    };
    let detail = match args.get("detail").map(|value| value.as_str()) {
        None | Some(Some("summary")) => "summary",
        Some(Some("full")) => "full",
        Some(_) => return mcp_error(id, -32602, "detail must be summary or full".into(), None),
    };
    // Continuation selects its historical publication before any active pin.
    if name == "neighbors" {
        // Validate before any backend work, then resolve a name argument.
        if let Err(error) = prepare_neighborhood(args.clone()) {
            return mcp_error(id, -32602, error, None);
        }
        if let Err(error) = resolve_names(id, name, repo, build_id, &mut args, connection).await {
            return error;
        }
        let query = match prepare_neighborhood(args) {
            Ok(query) => query,
            Err(error) => return mcp_error(id, -32602, error, None),
        };
        return match async {
            query
                .read(AnyReader::handle_cached(connection).await?)
                .await
        }
        .await
        {
            Ok(page) => mcp_text_result(id, neighborhood_budget(page, budget)),
            Err(error) => mcp_backend_error(id, name, &error),
        };
    }
    if name == "path" {
        let prepare = prepare_path;
        if let Err(error) = prepare(args.clone()) {
            return mcp_error(id, -32602, error, None);
        }
        if let Err(error) = resolve_names(id, name, repo, build_id, &mut args, connection).await {
            return error;
        }
        let query = prepare(args);
        let query = match query {
            Ok(query) => query,
            Err(error) => return mcp_error(id, -32602, error, None),
        };
        return match query.read(AnyReader::handle_cached(connection)).await {
            Ok(mut value) => {
                fit_budget(&mut value, budget, &["/path"]);
                mcp_text_result(id, value)
            }
            Err(error) => mcp_backend_error(id, name, &error),
        };
    }
    // Cheap service limits before any backend work.
    if args.get("max_tokens").is_some_and(|value| {
        !value
            .as_u64()
            .is_some_and(|tokens| (1..=MCP_MAX_TOKENS_MAX as u64).contains(&tokens))
    }) {
        return mcp_error(
            id,
            -32602,
            format!("max_tokens must be an integer in 1..{MCP_MAX_TOKENS_MAX}"),
            None,
        );
    }
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
    if matches!(name, "stats" | "community" | "report")
        && args
            .get("resolution")
            .is_some_and(|value| !value.as_f64().is_some_and(|r| (0.01..=10.0).contains(&r)))
    {
        return mcp_error(
            id,
            -32602,
            "resolution must be a number in 0.01..10".into(),
            None,
        );
    }
    let resolution = args
        .get("resolution")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(chaosbox_core::navigation::DEFAULT_RESOLUTION);
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
    if let Err(error) = resolve_names(id, name, repo, build_id, &mut args, connection).await {
        return error;
    }
    let reader = match async {
        AnyReader::pin(AnyReader::handle_cached(connection).await?, repo, build_id).await
    }
    .await
    {
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
                .map(|rows| serde_json::json!({"nodes": rows}))
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
            .stats_at(integer(args.get("limit"), 10), resolution)
            .await
            .map_err(|e| e.to_string()),
        "community" => reader
            .community_at(
                args["id"].as_str().unwrap_or_default(),
                integer(args.get("limit"), 100),
                resolution,
            )
            .await
            .map_err(|e| e.to_string()),
        "report" => reader
            .report(resolution)
            .await
            .map(
                |markdown| serde_json::json!({"build_id": reader.build_id(), "markdown": markdown}),
            )
            .map_err(|e| e.to_string()),
        "explain" => {
            let eid = args["id"].as_str().unwrap_or_default();
            reader.explain(eid).await.map_err(|e| e.to_string())
        }
        "review" => {
            let base = args.get("base").and_then(Value::as_str).unwrap_or("HEAD");
            review_payload(&reader, &enabled.root, base, budget).await
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
        Ok(v) => {
            let v = finish_payload(name, v, budget, enabled, &reader, include, detail).await;
            mcp_text_result(id, v)
        }
        Err(e) => mcp_error(id, -32603, e, None),
    }
}

#[cfg(test)]
pub(super) fn test_enabled() -> chaosbox::enablement::Enabled {
    chaosbox::enablement::Enabled {
        root: std::path::PathBuf::from("."),
        repo: "demo".into(),
    }
}

#[cfg(test)]
pub(super) async fn mcp_call_tool(id: &Value, name: &str, params: &Value) -> Value {
    let enabled = test_enabled();
    mcp_call_tool_connected(id, name, params, &mut None, Some(&enabled)).await
}

#[cfg(test)]
pub(super) async fn mcp_call_tool_without_opt_in(id: &Value, name: &str, params: &Value) -> Value {
    mcp_call_tool_connected(id, name, params, &mut None, None).await
}

/// Tools advertised in one `tools/list` page. Graph tools require the project
/// opt-in (`repo`); flagged operator tools stay visible without it.
pub(super) fn mcp_tool_list(
    repo: Option<&str>,
    intelligence: bool,
    federation: bool,
    workspace: bool,
) -> Vec<serde_json::Value> {
    let mut defs = mcp_tool_defs(repo);
    if intelligence {
        defs.push(default_repo_tool(mcp_tool("intelligence_context", "Small historical, source-backed knowledge packet; not instructions or current-state proof.", serde_json::json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"max_chars":{"type":"integer","minimum":256,"maximum":32000}}), vec!["query"]), repo));
        if federation {
            defs.push(default_repo_tool(mcp_tool(
                "intelligence_evidence",
                "Authorized source quotes and receipt projections from the exact originating provider/snapshot.",
                serde_json::json!({"handle":{"type":"object","additionalProperties":false,"properties":{
                    "provider":{"type":"string"},"owner":{"type":"string"},"scope":{"type":"string"},
                    "project":{"type":"string"},"snapshot":{"type":"string"},"id":{"type":"string"}},
                    "required":["provider","owner","scope","project","snapshot","id"]},
                    "max_chars":{"type":"integer","minimum":256,"maximum":32000}}),
                vec!["handle"],
            ), repo));
        } else {
            defs.push(default_repo_tool(
                mcp_tool(
                    "intelligence_evidence",
                    "Sources and typed decision receipts for a pinned intelligence item.",
                    serde_json::json!({"id":{"type":"string"}}),
                    vec!["id"],
                ),
                repo,
            ));
        }
    }
    if workspace {
        defs.push(default_repo_tool(mcp_tool("workspace_impact", "Reviewed directed impact, with current freshness, exact pins and private scope checks.",
            serde_json::json!({"changed":{"type":"string"},"scope":{"type":"string"},"max_hops":{"type":"integer","minimum":1,"maximum":8},"max_nodes":{"type":"integer","minimum":1,"maximum":100}}), vec!["changed", "scope"]), repo));
    }
    // Without the opt-in only explicitly flagged operator tools remain.
    if repo.is_none() {
        defs.retain(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("intelligence_") || name == "workspace_impact")
        });
    }
    defs
}

/// Read-only MCP over stdio: full handshake, one tool page, validated calls.
/// Never loads Jev credentials; never accepts prose as evidence.
// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
pub(super) async fn serve_mcp(
    intelligence: Option<chaosbox::intelligence::Bundle>,
    workspace: Option<chaosbox::workspace::Workspace>,
    mut current: Option<chaosbox::sync::current::CurrentReader>,
    federation: Option<chaosbox::federation::Federator>,
) -> std::io::Result<()> {
    let mut stdout = mcp_output()?;
    let mut frames = Frames::new(tokio::io::stdin());
    let mut initialized = false;
    // Explicit operator flags keep their own tools; graph tools need the
    // project's committed `.chaosbox.json` opt-in, resolved once at startup.
    let enabled = chaosbox::enablement::current();
    let repo = enabled.as_ref().map(|enabled| enabled.repo.as_str());
    // ponytail: one idle driver per ordinary server; no result/header cache.
    // Queued frames acquire credentials and fresh pins only when dispatched.
    let mut graph: Option<(PgReader, tokio::time::Instant)> = None;
    loop {
        let line = tokio::select! {
            biased;
            () = async { match graph.as_ref() {
                Some((_, deadline)) => tokio::time::sleep_until(*deadline).await,
                None => pending().await,
            }} => { graph = None; continue; }
            line = frames.request() => line?,
        };
        let Some((line, admission_deadline)) = line else {
            break;
        };
        let req = match decode_frame(&line) {
            Ok(Some(req)) => req,
            Ok(None) => continue,
            Err(response) => {
                if !exchange(&mut frames, &mut stdout, &Value::Null, async { response }).await? {
                    break;
                }
                continue;
            }
        };
        // Notifications carry no id and get no response.
        let id = match req.get("id") {
            Some(i) => i.clone(),
            None => continue,
        };
        if admission_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            if !exchange(&mut frames, &mut stdout, &id, async {
                admission_expired(&id)
            })
            .await?
            {
                break;
            }
            continue;
        }
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut response = async || match method {
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
                let mut result = serde_json::json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "chaosbox", "version": env!("CARGO_PKG_VERSION")},
                });
                if let Some(enabled) = enabled.as_ref() {
                    result["instructions"] =
                        serde_json::Value::String(mcp_instructions(&enabled.repo));
                } else {
                    result["instructions"] = serde_json::Value::String(MCP_NOT_ENABLED.to_owned());
                }
                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            "ping" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => {
                if initialized {
                    // One page, always: agents do not follow `nextCursor`.
                    let defs = mcp_tool_list(
                        repo,
                        intelligence.is_some() || current.is_some() || federation.is_some(),
                        federation.is_some(),
                        workspace.is_some(),
                    );
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"tools": defs}})
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                }
            }
            "tools/call" => {
                let started = std::time::Instant::now();
                let result = if initialized {
                    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name == "workspace_impact" {
                        graph = None;
                        match workspace.as_ref() {
                            Some(workspace) => {
                                let args = mcp_args(name, &params, repo);
                                match args.and_then(|args| {
                                    workspace_call(workspace, &Value::Object(args)).map_err(
                                        |error| serde_json::json!({"code": -32602, "message": error}),
                                    )
                                }) {
                                    Ok(value) => mcp_operator_result(&id, value, &params["arguments"]),
                                    Err(error) => mcp_error(
                                        &id,
                                        error["code"].as_i64().unwrap_or(-32602),
                                        error["message"]
                                            .as_str()
                                            .unwrap_or("invalid arguments")
                                            .to_owned(),
                                        None,
                                    ),
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
                        graph = None;
                        if let Some(reader) = federation.as_ref() {
                            match reader.query(name, &params["arguments"]).await {
                                Ok(value) => mcp_operator_result(&id, value, &params["arguments"]),
                                Err(error) => mcp_error(&id, -32602, error.to_string(), None),
                            }
                        } else if let Some(reader) = current.as_mut() {
                            match reader.query(name, &params["arguments"]).await {
                                Ok(value) => mcp_operator_result(&id, value, &params["arguments"]),
                                Err(error) => mcp_error(&id, -32602, error, None),
                            }
                        } else {
                            match intelligence.as_ref() {
                                Some(bundle) => match chaosbox::intelligence::mcp_query(
                                    bundle,
                                    name,
                                    &params["arguments"],
                                ) {
                                    Ok(value) => {
                                        mcp_operator_result(&id, value, &params["arguments"])
                                    }
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
                        // Move ownership into this future. Cancellation drops all
                        // request-owned clones; no cached driver survives it.
                        let mut connection = graph
                            .take()
                            .filter(|(_, deadline)| tokio::time::Instant::now() < *deadline)
                            .map(|(handle, _)| handle);
                        let result = mcp_call_tool_connected(
                            &id,
                            name,
                            &params,
                            &mut connection,
                            enabled.as_ref(),
                        )
                        .await;
                        if result.get("error").is_none() {
                            graph = connection.map(|handle| {
                                (handle, tokio::time::Instant::now() + MCP_GRAPH_IDLE_TIMEOUT)
                            });
                        }
                        result
                    }
                } else {
                    mcp_error(&id, -32600, "server not initialized".to_owned(), None)
                };
                log_tool_call(&params, started, &result);
                result
            }
            _ => mcp_error(
                &id,
                -32601,
                format!("unknown method (rejected): {method}"),
                None,
            ),
        };
        if !exchange(&mut frames, &mut stdout, &id, response()).await? {
            break;
        }
    }
    let _ = all_relation_types;
    Ok(())
}

/// Graphify accepts labels where Chaosbox wants ids: entity arguments that
/// are not ids resolve to exactly one entity in place. Ambiguous names fail
/// with their candidates; ids and unknown names pass through unchanged, so
/// a miss keeps each tool's historical result.
async fn resolve_names(
    id: &Value,
    name: &str,
    repo: &str,
    build_id: Option<&str>,
    args: &mut serde_json::Map<String, Value>,
    connection: &mut Option<PgReader>,
) -> Result<(), Value> {
    let keys: &[&str] = match name {
        "lookup" | "explain" => &["id"],
        "neighbors" if !args.contains_key("cursor") => &["id"],
        "path" => &["from", "to"],
        _ => &[],
    };
    let names: Vec<(&str, String)> = keys
        .iter()
        .filter_map(|key| {
            let value = args.get(*key)?.as_str()?;
            (!value.starts_with("ent:")).then(|| (*key, value.to_owned()))
        })
        .collect();
    if names.is_empty() {
        return Ok(());
    }
    let reader =
        async { AnyReader::pin(AnyReader::handle_cached(connection).await?, repo, build_id).await }
            .await
            .map_err(|e| mcp_backend_error(id, name, &e))?;
    for (key, value) in names {
        match reader.resolve(&value).await {
            Ok(chaosbox::Resolution::Entity(row)) => {
                args.insert(key.to_owned(), Value::String(row.entity_id));
            }
            Ok(chaosbox::Resolution::Ambiguous(candidates)) => {
                return Err(mcp_error(
                    id,
                    -32602,
                    format!("{key} {value:?} matches several entities; pass one id"),
                    Some(serde_json::json!({"code":"ambiguous","candidates":candidates})),
                ));
            }
            Ok(chaosbox::Resolution::Unknown(_)) => {}
            Err(e) => return Err(mcp_backend_error(id, name, &e)),
        }
    }
    Ok(())
}

/// Usage ledger for one `tools/call`: JSON-RPC errors keep their (negative)
/// code; successful payloads are re-parsed for build, size and count.
fn log_tool_call(params: &Value, started: std::time::Instant, result: &Value) {
    let name = params["name"].as_str().unwrap_or_default();
    let code = result["error"]["code"].as_i64().unwrap_or(0);
    // ponytail: re-parses the response text; hand the Value through if this
    // ever shows up in a profile.
    let payload = result["result"]["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    chaosbox::query_log::record(
        "mcp",
        name,
        &params["arguments"],
        started,
        code,
        payload.as_ref(),
    );
}

fn mcp_output() -> std::io::Result<Box<dyn AsyncWrite + Unpin>> {
    // Tokio Stdout uses an uncancellable blocking write. Reopen Linux pipes
    // nonblocking so EOF can stop an unread response without stranding a worker
    // during runtime shutdown. Keep normal terminal/file redirection support.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::FileTypeExt;
        if std::fs::metadata("/proc/self/fd/1")?.file_type().is_fifo() {
            return Ok(Box::new(
                tokio::net::unix::pipe::OpenOptions::new().open_sender("/proc/self/fd/1")?,
            ));
        }
    }
    Ok(Box::new(tokio::io::stdout()))
}

struct Queued {
    line: Vec<u8>,
    id: Value,
    deadline: tokio::time::Instant,
}

/// Cancellation-safe framing and bounded raw-request admission. No queued frame
/// owns credentials, a backend future, a publication pin or a query result.
pub(super) struct Frames<I> {
    input: BufReader<I>,
    line: Vec<u8>,
    queued: VecDeque<Queued>,
}

impl<I: AsyncRead + Unpin> Frames<I> {
    pub(super) fn new(input: I) -> Self {
        Self {
            input: BufReader::new(input),
            line: Vec::new(),
            queued: VecDeque::new(),
        }
    }

    async fn request(
        &mut self,
    ) -> std::io::Result<Option<(Vec<u8>, Option<tokio::time::Instant>)>> {
        if let Some(queued) = self.queued.pop_front() {
            return Ok(Some((queued.line, Some(queued.deadline))));
        }
        Ok(self.next().await?.map(|line| (line, None)))
    }

    pub(super) async fn next(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let remaining = MCP_FRAME_BYTES + 1 - self.line.len();
        let read = (&mut self.input)
            .take(remaining as u64)
            .read_until(b'\n', &mut self.line)
            .await?;
        if self.line.len() > MCP_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP input frame byte budget exceeded",
            ));
        }
        if read == 0 && self.line.is_empty() {
            return Ok(None);
        }
        if !self.line.ends_with(b"\n") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete MCP input frame",
            ));
        }
        Ok(Some(std::mem::take(&mut self.line)))
    }
}

fn decode_frame(line: &[u8]) -> Result<Option<Value>, Value> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let req: Value = serde_json::from_slice(line)
        .map_err(|_| mcp_error(&Value::Null, -32700, "parse error".into(), None))?;
    if !req.is_object()
        || req["jsonrpc"] != "2.0"
        || !req["method"].is_string()
        || req.get("id").is_some_and(|id| {
            !(id.is_i64()
                || id.is_u64()
                || id
                    .as_str()
                    .is_some_and(|s| s.len() <= 128 && !s.chars().any(char::is_control)))
        })
    {
        return Err(mcp_error(
            &Value::Null,
            -32600,
            "invalid request".into(),
            None,
        ));
    }
    Ok(Some(req))
}

struct Output {
    id: Value,
    bytes: Vec<u8>,
    offset: usize,
    deadline: tokio::time::Instant,
    final_response: bool,
}

impl Output {
    fn new(response: &Value, final_response: bool) -> Self {
        let mut bytes = chaosbox::read_view::bounded_json(response, MCP_RESPONSE_BYTES - 1)
            .unwrap_or_else(|_| {
                mcp_error(
                    &response["id"],
                    -32603,
                    "MCP response byte budget exceeded".into(),
                    None,
                )
                .to_string()
                .into_bytes()
            });
        bytes.push(b'\n');
        Self {
            id: response["id"].clone(),
            bytes,
            offset: 0,
            deadline: tokio::time::Instant::now() + MCP_OUTPUT_TIMEOUT,
            final_response,
        }
    }

    /// `write`, unlike `write_all`, is cancellation safe. Keep the offset and
    /// deadline outside the future so input cannot replay bytes or extend a write.
    async fn step<O: AsyncWrite + Unpin>(&mut self, output: &mut O) -> std::io::Result<bool> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "MCP output deadline exceeded",
            ));
        }
        tokio::time::timeout_at(self.deadline, async {
            if self.offset == self.bytes.len() {
                output.flush().await?;
                return Ok(true);
            }
            let written = output.write(&self.bytes[self.offset..]).await?;
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            self.offset += written;
            Ok(false)
        })
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "MCP output deadline exceeded")
        })?
    }
}

fn admission_expired(id: &Value) -> Value {
    mcp_error(
        id,
        -32000,
        "MCP admission wait deadline exceeded".into(),
        None,
    )
}

/// One backend future, one final response and bounded request/control queues.
/// EOF/error drops the in-place backend future; pings and cancellation bypass
/// admission. Controls drain before returning so every removed request receives
/// its response even when the active final response was already being written.
// ponytail: one active backend request per connection; increase concurrency only
// with explicit per-request connection ownership and the same cleanup bounds.
pub(super) async fn exchange<I, O, F>(
    frames: &mut Frames<I>,
    output: &mut O,
    id: &Value,
    response: F,
) -> std::io::Result<bool>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    F: Future<Output = Value>,
{
    let mut call = Some(Box::pin(response));
    let mut writing: Option<Output> = None;
    let mut ready: Option<Output> = None;
    let mut controls = VecDeque::<Output>::new();
    let mut finished = false;
    loop {
        if writing.is_none() {
            writing = controls.pop_front().or_else(|| ready.take());
        }
        if finished && writing.is_none() {
            return Ok(true);
        }
        let deadline = frames.queued.front().map(|queued| queued.deadline);
        let output_deadline = ready.as_ref().map(|frame| frame.deadline);
        tokio::select! {
            biased;
            value = async { match call.as_mut() { Some(future) => future.await, None => pending().await } } => {
                call = None;
                ready = Some(Output::new(&value, true));
            }
            written = async { match writing.as_mut() { Some(frame) => frame.step(output).await, None => pending().await } } => {
                if written? {
                    finished |= writing.take().is_some_and(|frame| frame.final_response);
                }
            }
            () = async { match output_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => pending().await,
            }} => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "MCP output deadline exceeded")),
            () = async { match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => pending().await,
            }} => {
                if controls.len() == MCP_PENDING_REQUESTS {
                    return Err(std::io::Error::other("MCP output backpressure: response queue full"));
                }
                if let Some(queued) = frames.queued.pop_front() {
                    controls.push_back(Output::new(&admission_expired(&queued.id), false));
                }
            }
            line = frames.next() => {
                let Some(line) = line? else { return Ok(false); };
                let response = match decode_frame(&line) {
                    Ok(None) => continue,
                    Err(error) => error,
                    Ok(Some(req)) => {
                        if req.get("id").is_none() {
                            if req["method"] != "notifications/cancelled" { continue; }
                            let Some(target) = req["params"].get("requestId") else { continue; };
                            if call.is_some() && target == id {
                                call = None;
                                ready = Some(Output::new(&mcp_error(id, -32800, "request cancelled".into(), None), true));
                                continue;
                            }
                            let Some(position) = frames.queued.iter().position(|queued| queued.id == *target) else { continue; };
                            let queued = frames.queued.remove(position).expect("queued position exists");
                            mcp_error(&queued.id, -32800, "request cancelled".into(), None)
                        } else if (!finished && req["id"] == *id)
                            || frames.queued.iter().any(|queued| queued.id == req["id"])
                            || writing.as_ref().is_some_and(|frame| frame.id == req["id"])
                            || controls.iter().any(|frame| frame.id == req["id"]) {
                            mcp_error(&Value::Null, -32600, "duplicate outstanding request id".into(), None)
                        } else if req["method"] == "ping" {
                            serde_json::json!({"jsonrpc":"2.0", "id":req["id"], "result":{}})
                        } else if frames.queued.len() < MCP_PENDING_REQUESTS
                            && frames.queued.iter().map(|queued| queued.line.len()).sum::<usize>() + line.len() <= MCP_PENDING_BYTES {
                            frames.queued.push_back(Queued { line, id: req["id"].clone(),
                                deadline: tokio::time::Instant::now() + MCP_ADMISSION_TIMEOUT });
                            continue;
                        } else {
                            mcp_error(&req["id"], -32000, "MCP admission queue full".into(), None)
                        }
                    }
                };
                if controls.len() == MCP_PENDING_REQUESTS {
                    return Err(std::io::Error::other("MCP output backpressure: response queue full"));
                }
                controls.push_back(Output::new(&response, false));
            }
        }
    }
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

/// Tool-level arguments never reach the strict query structs.
const MCP_QUERY_LOCAL_ARGS: [&str; 3] = ["max_tokens", "include", "detail"];

pub(super) fn prepare_path(
    mut args: serde_json::Map<String, serde_json::Value>,
) -> Result<chaosbox::PreparedPath, String> {
    for key in MCP_QUERY_LOCAL_ARGS {
        args.remove(key);
    }
    serde_json::from_value::<chaosbox::PathQuery>(Value::Object(args))
        .map_err(|error| error.to_string())
        .and_then(|query| query.prepare().map_err(|error| error.to_string()))
}

pub(super) fn prepare_neighborhood(
    mut args: serde_json::Map<String, serde_json::Value>,
) -> Result<chaosbox::PreparedNeighborhood, String> {
    for key in MCP_QUERY_LOCAL_ARGS {
        args.remove(key);
    }
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

// ---- Output budget, working-tree source, review ----

/// Character budget for one call: four characters per requested token.
pub(super) fn char_budget(args: &serde_json::Map<String, Value>) -> usize {
    args.get("max_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| usize::try_from(tokens).ok())
        .map_or(MCP_MAX_TOKENS_DEFAULT, |tokens| {
            tokens.min(MCP_MAX_TOKENS_MAX)
        })
        * MCP_CHARS_PER_TOKEN
}

fn encoded_len(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

fn list_len(payload: &Value, pointer: &str) -> usize {
    payload
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Trim whole items from the end of the given lists until the encoded payload
/// fits. The longest remaining list loses first, so every ranked list keeps its
/// head. `truncated` names every trimmed collection and keeps the first pass's
/// total, because a repeated pass sees only the trimmed length.
fn fit_budget(payload: &mut Value, budget: usize, lists: &[&str]) {
    let mut totals: std::collections::BTreeMap<&str, usize> = lists
        .iter()
        .map(|list| (*list, list_len(payload, list)))
        .collect();
    if let Some(previous) = payload.get("truncated").and_then(Value::as_object) {
        for (list, total) in &mut totals {
            if let Some(original) = previous
                .get(list.trim_start_matches('/'))
                .and_then(|entry| entry.get("total"))
                .and_then(Value::as_u64)
                .and_then(|total| usize::try_from(total).ok())
            {
                *total = original;
            }
        }
    }
    loop {
        record_truncated(payload, &totals);
        if encoded_len(payload) <= budget {
            return;
        }
        let mut victim: Option<&str> = None;
        let mut longest = 0;
        for list in lists {
            let len = list_len(payload, list);
            if len > longest {
                longest = len;
                victim = Some(list);
            }
        }
        let Some(victim) = victim else { return };
        if let Some(array) = payload.pointer_mut(victim).and_then(Value::as_array_mut) {
            array.pop();
        }
    }
}

/// Record the shown and original total of every collection that lost items,
/// keyed by its list name. Entries for collections trimmed by another pass stay.
fn record_truncated(payload: &mut Value, totals: &std::collections::BTreeMap<&str, usize>) {
    let mut entries = payload
        .get("truncated")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut trimmed = false;
    for (list, total) in totals {
        let shown = list_len(payload, list);
        if shown >= *total {
            continue;
        }
        entries.insert(
            list.trim_start_matches('/').to_owned(),
            serde_json::json!({"shown": shown, "total": total}),
        );
        trimmed = true;
    }
    if trimmed {
        payload["truncated"] = Value::Object(entries);
    }
}

/// Remove links whose endpoints did not survive budget trimming.
fn prune_links(payload: &mut Value) {
    let ids: std::collections::BTreeSet<String> = payload["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["id"].as_str().map(str::to_owned))
        .collect();
    if let Some(links) = payload["links"].as_array_mut() {
        links.retain(|link| {
            link["source"].as_str().is_some_and(|id| ids.contains(id))
                && link["target"].as_str().is_some_and(|id| ids.contains(id))
        });
    }
}

pub(super) fn trim_payload(name: &str, payload: &mut Value, budget: usize) {
    match name {
        "search" | "community" => fit_budget(payload, budget, &["/nodes"]),
        "context" | "export" => {
            fit_budget(payload, budget, &["/links", "/nodes"]);
            prune_links(payload);
        }
        "stats" => fit_budget(
            payload,
            budget,
            &[
                "/communities",
                "/hubs",
                "/god_nodes",
                "/questions",
                "/surprises",
            ],
        ),
        "diff" => fit_budget(
            payload,
            budget,
            &[
                "/added_nodes",
                "/removed_nodes",
                "/added_edges",
                "/removed_edges",
            ],
        ),
        "evidence" => fit_budget(payload, budget, &["/evidence"]),
        "status" => {
            // Summary already dropped the per-file list; `full` may still exceed
            // a tight budget, so the list goes first and names its omission.
            if encoded_len(payload) > budget {
                let total = list_len(payload, "/coverage/files");
                if total > 0 {
                    if let Some(files) = payload
                        .pointer_mut("/coverage/files")
                        .and_then(Value::as_array_mut)
                    {
                        files.clear();
                    }
                    record_truncated(
                        payload,
                        &std::collections::BTreeMap::from([("/coverage/files", total)]),
                    );
                }
            }
            fit_budget(payload, budget, &["/snapshots"]);
        }
        _ => {}
    }
}

/// Budget for the operator tools whose shapes this module does not own: trim
/// the longest top-level list, or leave scalar payloads untouched.
fn budget_any(mut value: Value, args: &Value) -> Result<Value, String> {
    let tokens = match args.get("max_tokens") {
        None => MCP_MAX_TOKENS_DEFAULT,
        Some(value) => value
            .as_u64()
            .and_then(|tokens| usize::try_from(tokens).ok())
            .filter(|tokens| (1..=MCP_MAX_TOKENS_MAX).contains(tokens))
            .ok_or_else(|| format!("max_tokens must be an integer in 1..{MCP_MAX_TOKENS_MAX}"))?,
    };
    let budget = tokens * MCP_CHARS_PER_TOKEN;
    let lists: Vec<String> = value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(_, value)| value.is_array())
                .map(|(key, _)| format!("/{key}"))
                .collect()
        })
        .unwrap_or_default();
    let pointers: Vec<&str> = lists.iter().map(String::as_str).collect();
    if !pointers.is_empty() {
        fit_budget(&mut value, budget, &pointers);
    }
    Ok(value)
}

/// Markdown has no item list: keep whole `##` sections and state the omission.
fn fit_report(payload: &mut Value, budget: usize) {
    let Some(markdown) = payload["markdown"].as_str().map(str::to_owned) else {
        return;
    };
    let mut sections: Vec<&str> = Vec::new();
    let mut start = 0;
    for (index, _) in markdown.match_indices("\n## ") {
        sections.push(&markdown[start..index]);
        start = index + 1;
    }
    sections.push(&markdown[start..]);
    let total = sections.len();
    while sections.len() > 1 && encoded_len(payload) > budget {
        sections.pop();
        let omitted = total - sections.len();
        payload["markdown"] = Value::String(format!(
            "{}\n\n[truncated: {omitted} of {total} sections omitted; raise max_tokens]\n",
            sections.join("\n")
        ));
    }
    if sections.len() < total {
        payload["truncated"] =
            serde_json::json!({"sections": {"shown": sections.len(), "total": total}});
    }
}

/// One entity span eligible for working-tree source.
pub(super) struct SourceTarget {
    pub(super) file: String,
    pub(super) snapshot: String,
    pub(super) start: u32,
    pub(super) end: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceInclude {
    Source,
    Signatures,
    None,
}

fn source_include(args: &serde_json::Map<String, Value>) -> Result<SourceInclude, String> {
    match args.get("include").map(Value::as_str) {
        None | Some(Some("source")) => Ok(SourceInclude::Source),
        Some(Some("signatures")) => Ok(SourceInclude::Signatures),
        Some(Some("none")) => Ok(SourceInclude::None),
        Some(_) => Err("include must be source, signatures or none".into()),
    }
}

/// Entities whose source an answer may embed, in ranked order.
fn source_targets(name: &str, payload: &Value) -> Vec<SourceTarget> {
    let entities: Vec<&Value> = match name {
        "context" => payload["nodes"]
            .as_array()
            .map(|nodes| nodes.iter().collect()),
        "lookup" | "explain" => payload.get("span").is_some().then(|| vec![payload]),
        _ => None,
    }
    .unwrap_or_default();
    entities
        .into_iter()
        .filter_map(|entity| {
            let file = entity["file"]
                .as_str()
                .or_else(|| entity["source_file"].as_str())?;
            let snapshot = entity["snapshot"].as_str()?;
            let span = entity.get("span")?;
            Some(SourceTarget {
                file: file.to_owned(),
                snapshot: snapshot.to_owned(),
                start: u32::try_from(span["start_line"].as_u64()?).ok()?,
                end: u32::try_from(span["end_line"].as_u64()?).ok()?,
            })
        })
        .collect()
}

/// Content digests recorded by the build, one bounded backend read per file.
async fn source_digests<R: chaosbox_store::GraphQueries>(
    reader: &chaosbox::GraphReader<R>,
    targets: &[SourceTarget],
) -> std::collections::BTreeMap<(String, String), String> {
    let mut digests = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for target in targets {
        let key = (target.snapshot.clone(), target.file.clone());
        if !seen.insert(key.clone()) {
            continue;
        }
        if seen.len() > MCP_SOURCE_FILES_MAX {
            break;
        }
        if let Ok(Some(sha256)) = reader.file_sha256(&target.snapshot, &target.file).await {
            digests.insert(key, sha256);
        }
    }
    digests
}

/// Repository-relative path resolved inside `root`; absolute paths, `..`,
/// and symlinks that leave the root are rejected.
pub(super) fn safe_join(root: &Path, file: &str) -> Option<PathBuf> {
    let relative = Path::new(file);
    if file.is_empty()
        || relative.is_absolute()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let canonical = std::fs::canonicalize(root.join(relative)).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn source_text(root: &Path, file: &str) -> Option<String> {
    let path = safe_join(root, file)?;
    std::fs::read_to_string(path).ok()
}

/// `cat -n`-style window; `None` when the span is empty or past the file.
fn number_span(text: &str, start: u32, end: u32) -> Option<(u32, u32, String)> {
    use std::fmt::Write as _;
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    if start == 0 || end < start {
        return None;
    }
    let mut numbered = String::new();
    let mut last = 0;
    for (index, line) in text
        .lines()
        .enumerate()
        .skip(start - 1)
        .take(end - start + 1)
    {
        writeln!(numbered, "{:>6}\t{line}", index + 1).ok()?;
        last = index + 1;
    }
    if last == 0 {
        return None;
    }
    Some((
        u32::try_from(start).ok()?,
        u32::try_from(last).ok()?,
        numbered,
    ))
}

fn pointer_entry(target: &SourceTarget, stale: bool) -> Value {
    let mut entry = serde_json::json!({
        "file": target.file, "start_line": target.start, "end_line": target.end,
    });
    if stale {
        entry["stale"] = Value::Bool(true);
    }
    entry
}

/// Line-numbered working-tree text in the common `source[]` shape, while the
/// answer budget lasts: full spans, then signatures, then pointers. Text is
/// served only when the recorded digest still matches the working tree.
pub(super) fn serve_source(
    root: &Path,
    targets: &[SourceTarget],
    digests: &std::collections::BTreeMap<(String, String), String>,
    include: SourceInclude,
    allowance: usize,
) -> (Vec<Value>, Vec<String>, Vec<String>) {
    let mut entries = Vec::new();
    let mut stale = Vec::new();
    let mut served = Vec::new();
    let mut cache: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut used = 0;
    for target in targets {
        if !seen.insert((target.file.clone(), target.start, target.end)) {
            continue;
        }
        let indexed = digests.get(&(target.snapshot.clone(), target.file.clone()));
        let text = cache
            .entry(target.file.clone())
            .or_insert_with(|| source_text(root, &target.file));
        let fresh = text.as_deref().is_some_and(|text| {
            indexed.is_some_and(|sha| chaosbox_core::sha256_hex(&[text]) == *sha)
        });
        if !fresh && indexed.is_some() {
            stale.push(target.file.clone());
        }
        let mut candidates = Vec::new();
        if fresh {
            let text = text.as_deref().unwrap_or_default();
            if include == SourceInclude::Source {
                if let Some((start, end, numbered)) = number_span(text, target.start, target.end) {
                    candidates.push(serde_json::json!({
                        "file": target.file, "start_line": start, "end_line": end, "text": numbered,
                    }));
                }
            }
            if include != SourceInclude::None {
                if let Some((start, _, numbered)) = number_span(text, target.start, target.start) {
                    candidates.push(serde_json::json!({
                        "file": target.file, "start_line": start, "end_line": start, "text": numbered,
                    }));
                }
            }
        }
        candidates.push(pointer_entry(target, !fresh && indexed.is_some()));
        let remaining = allowance.saturating_sub(used);
        let Some(entry) = candidates
            .into_iter()
            .find(|entry| encoded_len(entry) < remaining)
        else {
            break;
        };
        used += encoded_len(&entry) + 1;
        if entry.get("text").is_some() && !served.contains(&target.file) {
            served.push(target.file.clone());
        }
        entries.push(entry);
    }
    (entries, stale, served)
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_head(root: &Path) -> Option<String> {
    git_output(root, &["rev-parse", "HEAD"])
        .map(|head| head.trim().to_owned())
        .filter(|head| !head.is_empty())
}

/// Working-tree files whose tracked content differs from HEAD.
fn git_uncommitted(root: &Path, files: &[String]) -> Vec<String> {
    if files.is_empty() {
        return Vec::new();
    }
    let Some(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--untracked-files=no", "--"])
        .args(files)
        .output()
        .ok()
        .filter(|output| output.status.success())
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let path = line.get(3..)?;
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            Some(path.trim_matches('"').to_owned())
        })
        .collect()
}

/// Cheap local hint: the build records no commit, so compare the working tree
/// it was captured from with HEAD and with the recorded file digests.
fn freshness_hint(root: &Path, served: &[String], stale: &[String]) -> Option<Value> {
    let head = git_head(root)?;
    let uncommitted = git_uncommitted(root, served);
    if uncommitted.is_empty() && stale.is_empty() {
        return None;
    }
    let mut hint = serde_json::json!({"git_head": head});
    if !uncommitted.is_empty() {
        hint["uncommitted"] = serde_json::json!(uncommitted);
    }
    if !stale.is_empty() {
        hint["stale"] = serde_json::json!(stale);
    }
    hint["hint"] = Value::String(
        "Indexed source differs from this checkout; republish with `chaosbox run`.".into(),
    );
    Some(hint)
}

fn status_detail(payload: &mut Value, detail: &str, root: &Path) {
    if detail != "full" {
        if let Some(coverage) = payload.get_mut("coverage").and_then(Value::as_object_mut) {
            coverage.remove("files");
        }
    }
    if let Some(head) = git_head(root) {
        payload["commit"] = Value::String(head);
    }
    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .ok()
        .map(|status| !status.success());
    if let Some(dirty) = dirty {
        payload["freshness"] = serde_json::json!({"dirty": dirty});
    }
}

async fn finish_payload(
    name: &str,
    mut payload: Value,
    budget: usize,
    enabled: &chaosbox::enablement::Enabled,
    reader: &AnyReader,
    include: SourceInclude,
    detail: &str,
) -> Value {
    if name == "status" {
        status_detail(&mut payload, detail, &enabled.root);
    }
    if name == "report" {
        fit_report(&mut payload, budget);
    }
    trim_payload(name, &mut payload, budget);
    let targets = source_targets(name, &payload);
    if include != SourceInclude::None && !targets.is_empty() {
        let digests = source_digests(&**reader, &targets).await;
        let allowance = budget.saturating_sub(encoded_len(&payload) + 16);
        let (source, stale, served) =
            serve_source(&enabled.root, &targets, &digests, include, allowance);
        if !source.is_empty() {
            payload["source"] = Value::Array(source);
        }
        if let Some(hint) = freshness_hint(&enabled.root, &served, &stale) {
            payload["freshness"] = hint;
        }
        while encoded_len(&payload) > budget {
            let Some(entries) = payload["source"]
                .as_array_mut()
                .filter(|entries| !entries.is_empty())
            else {
                break;
            };
            entries.pop();
        }
        if payload["source"].as_array().is_some_and(Vec::is_empty) {
            if let Some(object) = payload.as_object_mut() {
                object.remove("source");
            }
        }
    }
    if encoded_len(&payload) > budget {
        trim_payload(name, &mut payload, budget);
    }
    payload
}

/// Changed entities by id, with the copy of the node they came from.
type ChangedEntities<'a> = std::collections::BTreeMap<String, (&'a Value, Vec<(u32, u32)>)>;

/// One changed file. Hunks are base-side line ranges (`-U0` old side), because
/// the indexed spans describe the base content, not the new file.
pub(super) struct ChangedFile {
    /// Path in the base tree; `None` for a file added since `base`, whose hunks
    /// can never map to an indexed span.
    pub(super) base: Option<String>,
    /// Reported path: the base path when it exists, else the added path.
    pub(super) file: String,
    pub(super) hunks: Vec<(u32, u32)>,
}

/// `git diff -U0 --no-ext-diff` against a verified revision, never a shell.
fn git_diff(root: &Path, base: &str) -> Result<(String, Vec<ChangedFile>), String> {
    let revision = format!("{base}^{{commit}}");
    let commit = git_output(
        root,
        &["rev-parse", "--verify", "--end-of-options", &revision],
    )
    .ok_or_else(|| format!("base {base:?} is not a commit in this repository"))?;
    let commit = commit.trim().to_owned();
    let diff = git_output(
        root,
        &["diff", "-U0", "--no-ext-diff", "--no-color", &commit],
    )
    .ok_or_else(|| "git diff failed".to_owned())?;
    Ok((commit, parse_diff(&diff)))
}

fn diff_path(value: &str) -> Option<String> {
    let path = value.split('\t').next().unwrap_or(value);
    if path == "/dev/null" {
        return None;
    }
    Some(
        path.trim_start_matches("a/")
            .trim_start_matches("b/")
            .to_owned(),
    )
}

/// Old-side lines of a `-U0` hunk header, so hunks quote the content the
/// indexed spans describe. A pure insertion (`b == 0`) has no old lines: its
/// insertion point `a` names the enclosing entity instead.
fn hunk_range(value: &str) -> Option<(u32, u32)> {
    let minus = value
        .split_whitespace()
        .find(|part| part.starts_with('-'))?;
    let mut parts = minus[1..].split(',');
    let start: u32 = parts.next()?.parse().ok()?;
    let count: u32 = parts.next().map_or(Ok(1), str::parse).ok()?;
    // `-0,0` inserts before the first line and encloses no entity.
    Some((start, start + count.saturating_sub(1)))
}

fn parse_diff(diff: &str) -> Vec<ChangedFile> {
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut old_path: Option<String> = None;
    let mut current: Option<ChangedFile> = None;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            files.extend(current.take());
            old_path = None;
        } else if let Some(path) = line.strip_prefix("--- ") {
            old_path = diff_path(path);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let base = old_path.take();
            if let Some(file) = base.clone().or_else(|| diff_path(path)) {
                files.extend(current.take());
                current = Some(ChangedFile {
                    base,
                    file,
                    hunks: Vec::new(),
                });
            }
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            if let (Some(current), Some(range)) = (current.as_mut(), hunk_range(rest)) {
                current.hunks.push(range);
            }
        }
    }
    files.extend(current);
    files.retain(|file| !file.hunks.is_empty());
    files
}

fn span_bounds(node: &Value) -> Option<(u32, u32)> {
    let span = node.get("span")?;
    Some((
        u32::try_from(span["start_line"].as_u64()?).ok()?,
        u32::try_from(span["end_line"].as_u64()?).ok()?,
    ))
}

/// Blob bytes at `commit:path`, `None` when the path is absent there. `commit`
/// is a full resolved sha, so the single object argument cannot name an option.
fn git_blob(root: &Path, commit: &str, path: &str) -> Option<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "blob", &format!("{commit}:{path}")])
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Base-side files whose bytes still match the digest the build recorded, using
/// the same content digest as the source-freshness check. Any other file was
/// changed since indexing, so its indexed spans cannot describe the hunks.
async fn verified_files<R: chaosbox_store::GraphQueries>(
    reader: &chaosbox::GraphReader<R>,
    root: &Path,
    commit: &str,
    graph: &Value,
    files: &[ChangedFile],
) -> std::collections::BTreeSet<String> {
    let mut snapshots = std::collections::BTreeMap::new();
    for node in graph["nodes"].as_array().into_iter().flatten() {
        if let (Some(file), Some(snapshot)) =
            (node["source_file"].as_str(), node["snapshot"].as_str())
        {
            snapshots.entry(file).or_insert(snapshot);
        }
    }
    let mut verified = std::collections::BTreeSet::new();
    for file in files {
        let (Some(base), Some(snapshot)) =
            (file.base.as_deref(), snapshots.get(file.file.as_str()))
        else {
            continue;
        };
        let Some(bytes) = git_blob(root, commit, base) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let digest = chaosbox_core::sha256_hex(&[text.as_ref()]);
        if reader
            .file_sha256(snapshot, base)
            .await
            .ok()
            .flatten()
            .as_deref()
            == Some(digest.as_str())
        {
            verified.insert(file.file.clone());
        }
    }
    verified
}

/// Map every hunk to its affected innermost entities: an entity that encloses
/// another affected entity is dropped, so one hunk may mark several siblings.
/// Files changed since indexing and uncovered ranges stay explicitly unmapped.
pub(super) fn changed_entities(
    graph: &Value,
    files: &[ChangedFile],
    verified: &std::collections::BTreeSet<String>,
) -> (Vec<Value>, Vec<Value>) {
    let nodes = graph["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let mut changed: ChangedEntities<'_> = std::collections::BTreeMap::new();
    let mut unmapped = Vec::new();
    for file in files {
        if !verified.contains(&file.file) {
            unmapped.push(serde_json::json!({"file": file.file, "reason": "stale"}));
            continue;
        }
        let candidates: Vec<&Value> = nodes
            .iter()
            .filter(|node| node["source_file"] == Value::String(file.file.clone()))
            .filter(|node| span_bounds(node).is_some())
            .collect();
        for &(start, end) in &file.hunks {
            let overlapping: Vec<(&&Value, (u32, u32))> = candidates
                .iter()
                .filter_map(|node| {
                    let bounds = span_bounds(node)?;
                    (bounds.0 <= end && bounds.1 >= start).then_some((node, bounds))
                })
                .collect();
            if overlapping.is_empty() {
                unmapped.push(serde_json::json!({
                    "file": file.file, "start_line": start, "end_line": end,
                }));
                continue;
            }
            for &(node, bounds) in &overlapping {
                let encloses_other = overlapping.iter().any(|&(_, other)| {
                    other != bounds && other.0 >= bounds.0 && other.1 <= bounds.1
                });
                if encloses_other {
                    continue;
                }
                let id = node["id"].as_str().unwrap_or_default().to_owned();
                changed
                    .entry(id)
                    .or_insert_with(|| (node, Vec::new()))
                    .1
                    .push((start, end));
            }
        }
    }
    let entities = changed
        .into_iter()
        .map(|(id, (node, mut ranges))| {
            ranges.sort_unstable();
            ranges.dedup();
            serde_json::json!({
                "id": id, "label": node["label"], "kind": node["kind"],
                "file": node["source_file"], "snapshot": node["snapshot"],
                "span": node["span"],
                "changed": ranges.iter().map(|(start, end)| serde_json::json!([start, end]))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    (entities, unmapped)
}

/// Incoming `calls`/`references` edges of one entity. Test-file callers are
/// reported separately: they are the regression safety net for a change.
pub(super) fn review_callers(graph: &Value, id: &str) -> (usize, Vec<Value>, Vec<Value>) {
    let nodes: std::collections::BTreeMap<&str, &Value> = graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut callers = Vec::new();
    let mut tests = Vec::new();
    for link in graph["links"].as_array().into_iter().flatten() {
        if link["target"] != Value::String(id.to_owned()) {
            continue;
        }
        let rel = link["rel_type"].as_str().unwrap_or_default();
        if rel != "calls" && rel != "references" {
            continue;
        }
        let Some(source) = link["source"].as_str() else {
            continue;
        };
        if !seen.insert(source) {
            continue;
        }
        let Some(node) = nodes.get(source) else {
            continue;
        };
        let caller = serde_json::json!({
            "id": source, "label": node["label"], "kind": node["kind"],
            "file": node["source_file"], "rel": rel,
        });
        if is_test_path(caller["file"].as_str().unwrap_or_default()) {
            tests.push(caller.clone());
        }
        callers.push(caller);
    }
    let order = |values: &mut Vec<Value>| {
        values.sort_by(|a, b| {
            a["file"]
                .as_str()
                .cmp(&b["file"].as_str())
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
    };
    order(&mut callers);
    order(&mut tests);
    (callers.len(), callers, tests)
}

pub(super) fn is_test_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.contains("tests/")
        || path.contains("test_")
        || path.contains("_test.")
        || path.contains(".test.")
        || path.contains("spec")
}

#[allow(clippy::too_many_lines)]
pub(super) async fn review_payload<R: chaosbox_store::GraphQueries>(
    reader: &chaosbox::GraphReader<R>,
    root: &Path,
    base: &str,
    budget: usize,
) -> Result<Value, String> {
    if base.is_empty() || base.len() > 256 || base.chars().any(char::is_control) {
        return Err("base must be 1..256 bytes without control characters".into());
    }
    let (commit, files) = git_diff(root, base)?;
    let graph = reader.export().await.map_err(|error| error.to_string())?;
    let verified = verified_files(reader, root, &commit, &graph, &files).await;
    let (entities, unmapped) = changed_entities(&graph, &files, &verified);
    let mut ranked: Vec<Value> = entities
        .into_iter()
        .map(|mut entity| {
            let id = entity["id"].as_str().unwrap_or_default().to_owned();
            let (count, callers, tests) = review_callers(&graph, &id);
            entity["callers"] = serde_json::json!(count);
            entity["caller_list"] = serde_json::json!(callers.iter().take(10).collect::<Vec<_>>());
            entity["tests"] = serde_json::json!(tests.iter().take(10).collect::<Vec<_>>());
            entity["test_count"] = serde_json::json!(tests.len());
            entity
        })
        .collect();
    // Risk order: most callers first, then id for determinism.
    ranked.sort_by(|a, b| {
        b["callers"]
            .as_u64()
            .cmp(&a["callers"].as_u64())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let mut payload = serde_json::json!({
        "build_id": reader.build_id, "base": base, "commit": commit,
        "changed_files": files.len(), "entities": ranked, "unmapped": unmapped,
    });
    fit_budget(&mut payload, budget, &["/entities", "/unmapped"]);
    // One changed region per entity per pass, risk order: a round-robin share
    // of whatever the metadata left over.
    let mut targets: Vec<SourceTarget> = Vec::new();
    let regions: Vec<Vec<SourceTarget>> = payload["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entity| {
            let file = entity["file"].as_str().unwrap_or_default().to_owned();
            let snapshot = entity["snapshot"].as_str().unwrap_or_default().to_owned();
            entity["changed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|range| {
                    let range = range.as_array()?;
                    Some(SourceTarget {
                        file: file.clone(),
                        snapshot: snapshot.clone(),
                        start: u32::try_from(range.first()?.as_u64()?).ok()?,
                        end: u32::try_from(range.get(1)?.as_u64()?).ok()?,
                    })
                })
                .collect()
        })
        .collect();
    let depth = regions.iter().map(Vec::len).max().unwrap_or(0);
    for index in 0..depth {
        for region in &regions {
            if let Some(target) = region.get(index) {
                targets.push(SourceTarget {
                    file: target.file.clone(),
                    snapshot: target.snapshot.clone(),
                    start: target.start,
                    end: target.end,
                });
            }
        }
    }
    let digests = source_digests(reader, &targets).await;
    let allowance = budget.saturating_sub(encoded_len(&payload) + 16);
    let (source, stale, served) =
        serve_source(root, &targets, &digests, SourceInclude::Source, allowance);
    if !source.is_empty() {
        payload["source"] = Value::Array(source);
    }
    if let Some(hint) = freshness_hint(root, &served, &stale) {
        payload["freshness"] = hint;
    }
    while encoded_len(&payload) > budget {
        let Some(entries) = payload["source"]
            .as_array_mut()
            .filter(|entries| !entries.is_empty())
        else {
            break;
        };
        entries.pop();
    }
    if payload["source"].as_array().is_some_and(Vec::is_empty) {
        if let Some(object) = payload.as_object_mut() {
            object.remove("source");
        }
    }
    if encoded_len(&payload) > budget {
        fit_budget(&mut payload, budget, &["/entities", "/unmapped"]);
    }
    Ok(payload)
}

fn neighborhood_budget(mut page: chaosbox::NeighborhoodPage, budget: usize) -> Value {
    let total = page.outgoing.len() + page.incoming.len();
    loop {
        let Ok(mut value) = serde_json::to_value(&page) else {
            return Value::Null;
        };
        let shown = page.outgoing.len() + page.incoming.len();
        if shown < total {
            let mut truncated = serde_json::json!({"shown": shown, "total": total});
            if let Some(cursor) = value.get("next_cursor").filter(|cursor| !cursor.is_null()) {
                truncated["next_cursor"] = cursor.clone();
            }
            value["truncated"] = serde_json::json!({"rows": truncated});
        }
        if encoded_len(&value) <= budget {
            return value;
        }
        let (out, inc) = (page.outgoing.len(), page.incoming.len());
        if out + inc == 0 {
            return value;
        }
        if out >= inc {
            page.trim_rows(out - 1, inc);
        } else {
            page.trim_rows(out, inc - 1);
        }
    }
}

fn mcp_backend_error(
    id: &serde_json::Value,
    name: &str,
    error: &impl std::fmt::Display,
) -> serde_json::Value {
    let message = error.to_string();
    let mut data = serde_json::json!(LifecycleReport::error(&format!("mcp {name}"), &message));
    // Machine-readable class for the one failure agents must act on
    // differently (index or pick another repo key) instead of retrying.
    if message.contains("no active build for repo") {
        data["code"] = serde_json::json!("no_active_build");
    }
    mcp_error(id, -32603, message, Some(data))
}

fn integer(value: Option<&serde_json::Value>, default: usize) -> usize {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(default)
}
