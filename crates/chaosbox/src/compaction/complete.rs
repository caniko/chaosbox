//! Complete-output recovery for file-backed bounded native results.
//!
//! The runtime may hand custody a preview while it keeps the complete bytes in
//! its own capture directory. Recovery is deliberately narrow: only regular,
//! bounded-size files named by the runtime's capture directories are read, and
//! an unrecoverable body stays bounded so reduction keeps failing closed.

use std::{fs, path::Path};
use serde_json::{json, Value};

/// Match the runtime snapshot ceiling so a recovered body stays capturable.
const LIMIT: u64 = 64 * 1024 * 1024;

/// Tools whose native `truncated` flag describes paging or a result limit.
/// Their settled result is already the complete output of that call.
const PAGED_TOOLS: [&str; 3] = ["read", "grep", "glob"];

pub(super) fn paged(tool: &str) -> bool {
    PAGED_TOOLS.contains(&tool)
}

fn capture_file(path: &str) -> Option<&Path> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let directory = path.parent()?.file_name()?.to_str()?;
    let shell_capture = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("out"));
    let recognized = (directory == "tool-output" && name.starts_with("tool_"))
        || (directory == "shell" && name.starts_with("sh_") && shell_capture);
    recognized.then_some(path)
}

/// Complete UTF-8 body referenced by a bounded result.
fn recover(path: &str) -> Option<String> {
    let path = capture_file(path)?;
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > LIMIT {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// The runtime's bounded-preview marker: `[showing ...; full output saved to PATH]`.
fn notice(text: &str) -> Option<String> {
    const MARKER: &str = "full output saved to ";
    let rest = &text[text.rfind(MARKER)? + MARKER.len()..];
    let end = rest.find([']', '\n', '\r']).unwrap_or(rest.len());
    let path = rest[..end].trim();
    (!path.is_empty()).then(|| path.to_owned())
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter(|part| part["type"] == "text")
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

/// Replace the bounded text parts with the complete body, keeping file parts.
fn merged(content: Option<&Value>, body: String) -> Value {
    let mut remaining = Some(body);
    let mut merged = Vec::new();
    for part in content.and_then(Value::as_array).into_iter().flatten() {
        if part["type"] != "text" {
            merged.push(part.clone());
            continue;
        }
        if let Some(body) = remaining.take() {
            merged.push(json!({"type":"text","text":body}));
        }
    }
    if let Some(body) = remaining {
        merged.insert(0, json!({"type":"text","text":body}));
    }
    Value::Array(merged)
}

/// Complete variant of a bounded tool-result record.
pub(super) fn tool(record: &Value) -> Option<Value> {
    let metadata = record.pointer("/result/metadata")?;
    // Only a result the runtime marked bounded can have lost bytes.
    if metadata.get("truncated").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    if paged(record["tool"].as_str().unwrap_or_default()) {
        return None;
    }
    let path = if let Some(path) = metadata.get("outputPath").and_then(Value::as_str) {
        path.to_owned()
    } else {
        let text = text_of(&record["result"]["content"])
            .or_else(|| text_of(&record["result"]["output"]))?;
        notice(&text)?
    };
    let body = recover(&path)?;
    let mut complete = record.clone();
    complete["result"]["content"] = merged(record.pointer("/result/content"), body);
    let metadata = complete.pointer_mut("/result/metadata")?.as_object_mut()?;
    metadata.insert("truncated".into(), json!(false));
    metadata.remove("outputPath");
    Some(complete)
}

/// Complete variant of a bounded shell session record.
pub(super) fn shell(record: &Value) -> Option<Value> {
    let output = record.get("output")?;
    if output.get("truncated").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let path = record
        .get("outputPath")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            output
                .get("output")
                .and_then(Value::as_str)
                .and_then(notice)
        })?;
    let body = recover(&path)?;
    let mut complete = record.clone();
    complete["output"]["output"] = json!(body);
    complete["output"]["truncated"] = json!(false);
    complete.as_object_mut()?.remove("outputPath");
    Some(complete)
}
