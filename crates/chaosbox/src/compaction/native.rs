//! Checked views of native JSON. The immutable archive retains every original field.
use std::collections::BTreeSet;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Kind {
    User,
    Assistant,
    ToolResult,
    Shell,
    System,
    Synthetic,
    Compaction,
    Agent,
    AgentSwitched,
    ModelSwitched,
    ModelSelected,
    LocationSwitched,
    Skill,
    Idle,
}

#[derive(Deserialize)]
struct Header<'a> {
    #[serde(borrow)]
    id: &'a str,
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(rename = "sessionID", borrow)]
    session: Option<&'a str>,
}

pub(super) struct Record<'a> {
    pub id: &'a str,
    pub kind: Kind,
    pub settled: bool,
}

impl<'a> Record<'a> {
    pub fn parse(raw: &'a Value, session: &str) -> Result<Self, String> {
        let header = Header::deserialize(raw).map_err(|e| format!("invalid native record: {e}"))?;
        if header.id.trim().is_empty() || header.session.is_some_and(|s| s != session) {
            return Err("empty or cross-session native identity".into());
        }
        let settled = match header.kind {
            Kind::Assistant => assistant(raw)?,
            Kind::User => {
                if raw.get("text").is_some() {
                    string(raw, "text")?;
                } else {
                    let parts = raw
                        .pointer("/prompt/parts")
                        .and_then(Value::as_array)
                        .ok_or("native user lacks text or prompt parts")?;
                    for part in parts {
                        match string(part, "type")? {
                            "text" => {
                                string(part, "text")?;
                            }
                            "file" => (), // Inline custody is checked before projection.
                            _ => return Err("unsupported native prompt part".into()),
                        }
                    }
                }
                true
            }
            Kind::ToolResult => {
                for key in ["messageID", "callID", "tool"] {
                    identity(raw, key)?;
                }
                object(raw, "input")?;
                match string(raw, "status")? {
                    "completed" => {
                        let result = object(raw, "result")?;
                        if let Some(content) = result.get("content") {
                            tool_content(content)?;
                        } else if !result.contains_key("output") {
                            return Err("tool result lacks its complete outcome".into());
                        }
                    }
                    "error" => {
                        string(
                            raw.get("error").ok_or("tool settlement lacks error")?,
                            "message",
                        )?;
                    }
                    _ => return Err("invalid settled tool result".into()),
                }
                true
            }
            Kind::Shell => shell(raw)?,
            _ => true, // Control records remain opaque, retained original data.
        };
        Ok(Self {
            id: header.id,
            kind: header.kind,
            settled,
        })
    }
}

fn shell(raw: &Value) -> Result<bool, String> {
    string(raw, "command")?;
    match string(raw, "status")? {
        "running" => Ok(false),
        "completed" | "error" | "exited" | "timeout" | "killed" => {
            let output = raw.get("output").ok_or("native shell lacks outcome")?;
            if !output.is_string() {
                string(output, "output")?;
                output
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .ok_or("native shell truncation must be explicit")?;
            }
            Ok(true)
        }
        _ => Err("unsupported native shell status".into()),
    }
}

fn assistant(raw: &Value) -> Result<bool, String> {
    if raw.get("parts").is_some() && raw.get("content").is_some() {
        return Err("ambiguous native assistant content".into());
    }
    let parts = raw
        .get("parts")
        .or_else(|| raw.get("content"))
        .and_then(Value::as_array)
        .ok_or("native assistant lacks content array")?;
    let mut settled = raw
        .pointer("/time/completed")
        .and_then(Value::as_i64)
        .is_some();
    let mut calls = BTreeSet::new();
    for part in parts {
        match string(part, "type")? {
            "text" | "reasoning" => {
                string(part, "text")?;
            }
            "tool" => {
                let id = identity(part, "id")?;
                identity(part, "name")?;
                if !calls.insert(id) {
                    return Err("duplicate native tool call".into());
                }
                let state = part.get("state").ok_or("native tool lacks state")?;
                match string(state, "status")? {
                    "streaming" | "pending" | "running" => settled = false,
                    "completed" => {
                        object(state, "input")?;
                        tool_content(state.get("content").ok_or("native tool lacks outcome")?)?;
                    }
                    "error" => {
                        object(state, "input")?;
                        string(
                            state.get("error").ok_or("native tool lacks error")?,
                            "message",
                        )?;
                        if let Some(content) = state.get("content") {
                            tool_content(content)?;
                        }
                    }
                    _ => return Err("unsupported native tool status".into()),
                }
            }
            _ => return Err("unsupported native assistant part".into()),
        }
    }
    Ok(settled)
}

fn string<'a>(raw: &'a Value, key: &str) -> Result<&'a str, String> {
    raw.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("native {key} must be a string"))
}

fn identity<'a>(raw: &'a Value, key: &str) -> Result<&'a str, String> {
    let value = string(raw, key)?;
    if value.trim().is_empty() {
        return Err(format!("native {key} must not be empty"));
    }
    Ok(value)
}

fn object<'a>(raw: &'a Value, key: &str) -> Result<&'a serde_json::Map<String, Value>, String> {
    raw.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("native {key} must be an object"))
}

fn tool_content(content: &Value) -> Result<(), String> {
    if content.is_string() {
        return Ok(());
    }
    let parts = content
        .as_array()
        .ok_or("tool outcome must contain text or content parts")?;
    for part in parts {
        match string(part, "type")? {
            "text" => {
                string(part, "text")?;
            }
            "file" => {
                string(part, "uri")?;
            }
            _ => return Err("unsupported native tool content".into()),
        }
    }
    Ok(())
}

pub(super) fn complete_result(raw: &Value) -> Result<(), String> {
    if raw
        .pointer("/result/metadata/truncated")
        .is_some_and(|truncated| truncated != &Value::Bool(false))
        || raw.pointer("/result/metadata/outputPath").is_some()
    {
        return Err("tool was bounded before capture; complete output lacks custody".into());
    }
    Ok(())
}
