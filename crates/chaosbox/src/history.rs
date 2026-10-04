//! Reversible runtime projections for archived, unfinished assistant records.
//! Never call a model or replay a tool. The exact original remains an archive
//! artifact; settlement below describes the import copy, not task completion.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Version the transformation independently of source/runtime versions.
pub const DRAFT_VERSION: &str = "interrupted-draft-v1";

/// Exact archival record plus its explicitly interrupted runtime projection.
#[derive(Debug, Serialize, Deserialize)]
pub struct DraftProjection {
    /// False for records already settled; their projection is unchanged.
    pub changed: bool,
    /// Canonical JSON digest, using Chaosbox's framed SHA-256 identity.
    pub original_digest: String,
    /// Original parsed record, preserved without deleting provider/tool state.
    pub original: Value,
    /// Native v2-compatible projection, never a fabricated successful result.
    pub message: Value,
}

/// Project one unfinished assistant into an importable interrupted draft.
/// `settled_at` is the explicit migration settlement time, not an inferred
/// original completion time; callers must journal it for deterministic resume.
pub fn recover_draft(
    original: Value,
    snapshot: &str,
    settled_at: i64,
) -> Result<DraftProjection, String> {
    if snapshot.len() != 64 || !snapshot.bytes().all(|b| b.is_ascii_hexdigit()) || settled_at < 0 {
        return Err("snapshot must be a SHA-256 digest and settlement time non-negative".into());
    }
    let id = original
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("msg_"))
        .ok_or("record lacks native message identity")?;
    let kind = original
        .get("type")
        .and_then(Value::as_str)
        .ok_or("record lacks type")?;
    let digest = chaosbox_core::sha256_hex(&[
        &serde_json::to_string(&original).map_err(|_| "encode source")?
    ]);
    if kind != "assistant" {
        if original.get("status").and_then(Value::as_str) == Some("running") {
            return Err(
                "unfinished non-assistant operation requires a separate reviewed adapter".into(),
            );
        }
        return Ok(DraftProjection {
            changed: false,
            original_digest: digest,
            message: original.clone(),
            original,
        });
    }
    if original
        .pointer("/time/completed")
        .is_some_and(|v| !v.is_null())
    {
        return Ok(DraftProjection {
            changed: false,
            original_digest: digest,
            message: original.clone(),
            original,
        });
    }
    let created = original
        .pointer("/time/created")
        .and_then(Value::as_i64)
        .ok_or("assistant lacks creation time")?;
    if settled_at < created {
        return Err("migration settlement predates source creation".into());
    }
    let mut message = original.clone();
    let parts = message
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .ok_or("assistant content must be an array")?;
    let mut interrupted_tools = Vec::new();
    for (index, part) in parts.iter_mut().enumerate() {
        if interrupt_tool(part)? {
            interrupted_tools.push(index);
        }
    }
    let notice_part = parts.len();
    parts.push(json!({"type":"text","text":"[History migration notice] This assistant response was unfinished in the archived snapshot. Its partial text and observations are retained as an interrupted draft, not a successful completion. Tool side effects may have occurred; verify current state before retrying. Nothing was replayed. Exact original records remain in the migration archive."}));
    let object = message.as_object_mut().ok_or("message must be an object")?;
    object.remove("retry");
    object.remove("rawFinish");
    object.insert("finish".into(), json!("error"));
    object.insert("error".into(), json!({"type":"history_snapshot_incomplete","message":"Interrupted archival draft; original completion is unknown."}));
    let time = object
        .get_mut("time")
        .and_then(Value::as_object_mut)
        .ok_or("time must be an object")?;
    time.insert("completed".into(), json!(settled_at));
    let metadata = object
        .entry("metadata")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("metadata must be an object")?;
    if metadata.contains_key("chaosboxMigrationDraft") {
        return Err("source already contains a migration draft marker".into());
    }
    metadata.insert(
        "chaosboxMigrationDraft".into(),
        json!({
            "version":DRAFT_VERSION,"sourceSnapshot":snapshot,"sourceMessageID":id,
            "originalDigest":digest,"originalTime":original.get("time"),
            "settledForImportAt":settled_at,"originalCompletionKnown":false,
            "noticePart":notice_part,"interruptedToolParts":interrupted_tools,
        }),
    );
    Ok(DraftProjection {
        changed: true,
        original_digest: digest,
        original,
        message,
    })
}

fn interrupt_tool(part: &mut Value) -> Result<bool, String> {
    if part.get("type").and_then(Value::as_str) != Some("tool") {
        return Ok(false);
    }
    let state = part.get_mut("state").ok_or("tool lacks state")?;
    match state.get("status").and_then(Value::as_str) {
        Some("completed" | "error") => Ok(false),
        Some("running" | "streaming") => {
            let input = state
                .get("input")
                .cloned()
                .ok_or("unfinished tool lacks input")?;
            let input = if input.is_object() {
                input
            } else {
                json!({"_archivedPartialInput":input})
            };
            let mut replacement = json!({"status":"error","input":input,"error":{
                "type":"history_snapshot_incomplete",
                "message":"No completion was recorded in the archived snapshot. Side effects are unknown; inspect current state before retrying. No tool was replayed by migration."
            }});
            for key in ["metadata", "content"] {
                if let Some(value) = state.get(key) {
                    replacement[key] = value.clone();
                }
            }
            *state = replacement;
            Ok(true)
        }
        _ => Err("unknown tool status; refusing to guess a projection".into()),
    }
}
