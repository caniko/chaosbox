use std::fmt::Write as _;
use chaosbox_core::sha256_hex;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use super::{err, Capture, Journal, Plan};

/// Project source text/tool fields into inference, excluding provider state,
/// reasoning, system content and generated checkpoints.
pub(super) fn normalized(input: &Capture) -> Result<String, String> {
    let mut lines = Vec::new();
    for record in &input.records {
        let mut value = json!({"id":record["id"],"type":record["type"],"time":record["time"]});
        if let Some(draft) = record.pointer("/metadata/chaosboxMigrationDraft") {
            value["metadata"] = json!({"chaosboxMigrationDraft":draft});
        }
        match record["type"].as_str() {
            Some("user") => {
                let parts = record.pointer("/prompt/parts").and_then(Value::as_array);
                let text = record["text"].as_str().map_or_else(
                    || {
                        parts
                            .into_iter()
                            .flatten()
                            .filter(|p| p["type"] == "text")
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    },
                    str::to_owned,
                );
                value["text"] = json!(text);
            }
            Some("assistant") => {
                let content: Vec<_> = record["parts"]
                    .as_array()
                    .or_else(|| record["content"].as_array())
                    .into_iter()
                    .flatten()
                    .filter(|p| matches!(p["type"].as_str(), Some("text" | "tool")))
                    .filter(|p| {
                        !p["name"]
                            .as_str()
                            .is_some_and(|n| n.starts_with("chaosbox_"))
                    })
                    .map(|p| {
                        if p["type"] == "text" {
                            json!({"type":"text","text":p["text"]})
                        } else {
                            json!({"type":"tool","id":p["id"],"name":p["name"],"state":p["state"]})
                        }
                    })
                    .collect();
                value["content"] = json!(content);
            }
            Some("shell") => {
                value["command"] = record["command"].clone();
                value["status"] = record["status"].clone();
                value["exit"] = record["exit"].clone();
                value["output"] = record["output"].clone();
            }
            _ => continue,
        }
        lines.push(serde_json::to_string(&value).map_err(err)?);
    }
    Ok(lines.join("\n"))
}

impl Journal {
    /// Reconcile early tool bodies into their settled native message group.
    /// Early capture alone is custody, not a second independent occurrence.
    pub(super) fn normalized(&self, input: &Capture) -> Result<String, String> {
        let mut complete = input.clone();
        for record in &mut complete.records {
            if record["type"] != "assistant" {
                continue;
            }
            let message = record["id"]
                .as_str()
                .ok_or("missing native identity")?
                .to_owned();
            let key = if record["parts"].is_array() {
                "parts"
            } else {
                "content"
            };
            for part in record[key].as_array_mut().into_iter().flatten() {
                if part["type"] != "tool" {
                    continue;
                }
                if let Some((_, original)) = self.full_tool(input, &message, part)? {
                    part["state"]["content"] = tool_content(&original);
                    part["state"]["metadata"] = original
                        .pointer("/result/metadata")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                }
            }
        }
        normalized(&complete)
    }

    fn full_tool(
        &self,
        capture: &Capture,
        message: &str,
        part: &Value,
    ) -> Result<Option<(String, Value)>, String> {
        let full: Option<String> = self.db.query_row("SELECT hash FROM tool_sources WHERE source=?1 AND session=?2 AND repo=?3 AND message=?4 AND call_id=?5",
            params![capture.source,capture.session,capture.repo,message,part["id"].as_str()], |r| r.get(0)).optional().map_err(err)?;
        full.map(|hash| {
            let original = self.evidence(&hash)?;
            if original["tool"] != part["name"]
                || original["input"] != part["state"]["input"]
                || original["status"] != part["state"]["status"]
            {
                return Err("tool custody does not match native settlement".into());
            }
            Ok((hash, original))
        })
        .transpose()
    }

    /// Build a conservative checkpoint. Only high-confidence irrelevant
    /// assistant text can be omitted; tool groups and users remain.
    pub fn plan(&mut self, capture_id: &str, max_chars: usize) -> Result<Plan, String> {
        if !(512..=120_000).contains(&max_chars) {
            return Err("invalid checkpoint character budget".into());
        }
        let capture = self.load(capture_id)?;
        let text = self.normalized(&capture)?;
        let latest = text
            .lines()
            .filter_map(|line| {
                serde_json::from_str::<Value>(line)
                    .ok()
                    .filter(|r| r["type"] == "user")
                    .map(|_| line)
            })
            .next_back()
            .ok_or("checkpoint requires a native user anchor")?;
        let mut summary = format!(
            "# Source-backed continuation\n\nOriginal user requirements and unresolved work follow. Assistant text is historical assertion; verify current repository state.\nSource custody: {capture_id}\nLarge outputs: retrieve with chaosbox_archive(hash).\n"
        );
        let mut externalized = Vec::new();
        let mut omitted = Vec::new();
        for record in &capture.records {
            let raw = serde_json::to_string(record).map_err(err)?;
            let hash = sha256_hex(&[&raw]);
            let kind = record["type"].as_str().ok_or("missing native kind")?;
            if matches!(kind, "compaction" | "tool-result") {
                continue;
            }
            if self.can_omit(&capture, record, &hash, latest, &text)? {
                omitted.push(hash);
                continue;
            }
            let projection = self.project_record(&capture, record, &hash, &mut externalized)?;
            writeln!(
                summary,
                "\n## {kind} [{}; source {hash}]\n{}",
                record["id"].as_str().ok_or("missing id")?,
                serde_json::to_string(&projection).map_err(err)?
            )
            .map_err(err)?;
        }
        if summary.chars().count() > max_chars {
            return Err("protected continuation exceeds budget; custody retained, context reduction refused".into());
        }
        let id = sha256_hex(&["session-pruning-v1", capture_id, &summary]);
        let plan = Plan {
            summary,
            recent: String::new(),
            custody: capture_id.into(),
            id,
            externalized,
            omitted,
        };
        self.db
            .execute(
                "INSERT OR IGNORE INTO plans VALUES (?1,?2,?3)",
                params![
                    plan.id,
                    capture_id,
                    serde_json::to_string(&plan).map_err(err)?
                ],
            )
            .map_err(err)?;
        Ok(plan)
    }

    fn project_record(
        &self,
        capture: &Capture,
        record: &Value,
        hash: &str,
        externalized: &mut Vec<String>,
    ) -> Result<Value, String> {
        let mut projection = record.clone();
        if let Some(object) = projection.as_object_mut() {
            object.remove("providerState");
            object.remove("tokens");
            object.remove("cost");
        }
        if let Some(files) = projection["files"].as_array_mut() {
            for (index, file) in files.iter_mut().enumerate() {
                if !file["data"].is_string() {
                    return Err("attachment bytes are not under custody".into());
                }
                file["data"] = json!(format!(
                    "Archived attachment: chaosbox_archive({hash}), pointer /files/{index}/data"
                ));
                if !externalized.iter().any(|h| h == hash) {
                    externalized.push(hash.to_owned());
                }
            }
        }
        let parts_key = if projection["parts"].is_array() {
            "parts"
        } else {
            "content"
        };
        self.project_parts(
            capture,
            record,
            &mut projection[parts_key],
            hash,
            externalized,
        )?;
        if record["type"] == "shell"
            && projection
                .pointer("/output/truncated")
                .and_then(Value::as_bool)
                == Some(true)
        {
            return Err("truncated shell output needs complete custody before reduction".into());
        }
        if record["type"] == "shell"
            && serde_json::to_vec(&projection["output"])
                .map_err(err)?
                .len()
                > 2000
        {
            projection["output"] = json!({"archive_hash":hash,"externalized":true});
            externalized.push(hash.to_owned());
        }
        Ok(projection)
    }

    fn can_omit(
        &self,
        capture: &Capture,
        record: &Value,
        hash: &str,
        latest: &str,
        text: &str,
    ) -> Result<bool, String> {
        let parts_key = if record["parts"].is_array() {
            "parts"
        } else {
            "content"
        };
        let has_tools = record[parts_key]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool"));
        if record["type"] != "assistant" || has_tools {
            return Ok(false);
        }
        let anchor = sha256_hex(&[latest]);
        let receipt: Option<String> = self
            .db
            .query_row(
                "SELECT receipt FROM selections WHERE hash=?1 AND anchor=?2",
                params![hash, anchor],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        if let Some(raw) = receipt {
            let (input, response): (crate::continuation::Input, chaosbox_jev::SystemOneResponse) =
                serde_json::from_str(&raw).map_err(err)?;
            if input.scope != capture.scope
                || input.repo != capture.repo
                || input.session != capture.session
                || input.source != capture.source
                || input.latest_user.raw != latest
                || input.records.len() != 1
                || input.records[0].id != record["id"]
            {
                return Err("invalid source-bound pruning receipt".into());
            }
            let projection = text
                .lines()
                .find(|line| {
                    serde_json::from_str::<Value>(line).is_ok_and(|r| r["id"] == record["id"])
                })
                .ok_or("missing pruning source")?;
            if input.records[0].raw != projection {
                return Err("pruning source digest changed".into());
            }
            let selected = crate::continuation::render(&input, &response, 120_000)?;
            if selected["omitted"]
                .as_array()
                .is_some_and(|r| !r.is_empty())
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn project_parts(
        &self,
        capture: &Capture,
        record: &Value,
        content: &mut Value,
        hash: &str,
        externalized: &mut Vec<String>,
    ) -> Result<(), String> {
        if let Some(parts) = content.as_array_mut() {
            parts.retain(|p| p["type"] != "reasoning");
            for part in parts {
                if let Some(object) = part.as_object_mut() {
                    object.remove("providerState");
                    object.remove("providerResultState");
                    if object.get("type").is_some_and(|v| v == "text") {
                        object.remove("state");
                    }
                }
                if part["type"] == "tool" {
                    let full = self.full_tool(
                        capture,
                        record["id"].as_str().ok_or("missing native identity")?,
                        part,
                    )?;
                    let was_truncated = part
                        .pointer("/state/metadata/truncated")
                        .and_then(Value::as_bool)
                        == Some(true)
                        || part.pointer("/state/metadata/outputPath").is_some();
                    if was_truncated && full.is_none() {
                        return Err(
                            "complete tool output lacks durable pre-bounding custody".into()
                        );
                    }
                    if let Some((full_hash, original)) = full {
                        if original.pointer("/result/metadata/outputPath").is_some() {
                            return Err(
                                "tool was bounded before capture; complete output lacks custody"
                                    .into(),
                            );
                        }
                        require_inline_files(&tool_content(&original))?;
                        part["state"]["archive_hash"] = json!(full_hash);
                    }
                    require_inline_files(&part["state"]["content"])?;
                    let content = &part["state"]["content"];
                    if serde_json::to_vec(content).map_err(err)?.len() > 2000 {
                        let output_hash = part["state"]["archive_hash"]
                            .as_str()
                            .unwrap_or(hash)
                            .to_owned();
                        part["state"]["content"] = json!([{"type":"text","text":format!("Complete tool result externalized: chaosbox_archive({output_hash}); no success inferred from this reference.")}]);
                        if !externalized.contains(&output_hash) {
                            externalized.push(output_hash);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn tool_content(record: &Value) -> Value {
    match record.pointer("/result/content") {
        Some(Value::String(text)) => json!([{"type":"text","text":text}]),
        Some(Value::Array(content)) => json!(content),
        _ => {
            let text = record
                .pointer("/result/output")
                .map(Value::to_string)
                .or_else(|| {
                    record
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            text.map_or_else(|| json!([]), |text| json!([{"type":"text","text":text}]))
        }
    }
}

fn require_inline_files(content: &Value) -> Result<(), String> {
    for part in content.as_array().into_iter().flatten() {
        if part["type"] == "file"
            && !part["uri"]
                .as_str()
                .is_some_and(|uri| uri.starts_with("data:"))
        {
            return Err("tool attachment bytes lack durable custody; reduction refused".into());
        }
    }
    Ok(())
}
