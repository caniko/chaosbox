use chaosbox_core::{
    intelligence::{IntelligenceCandidate, SessionEvidence},
    sha256_hex,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bounded staging catalog. Omitted proposals are explicit coverage, not
/// silently accepted or discarded evidence. Compactions are not new sources.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidates {
    /// Artifact format version.
    pub version: u32,
    /// Visibility boundary.
    pub scope: String,
    /// Digest of the complete input, not only selected lines.
    pub snapshot: String,
    /// Proposals awaiting typed assessment.
    pub candidates: Vec<IntelligenceCandidate>,
    /// Eligible lines beyond the requested cap.
    pub omitted: usize,
    /// Records excluded because they are summaries or synthetic/system data.
    pub excluded_derived: usize,
    /// Qualifying candidates deliberately skipped by the requested window.
    pub skipped: usize,
    /// Additional bounded candidates remain after this window.
    pub has_more: bool,
}

/// Extract copied lines from normalized `OpenCode` JSONL. No model, no file
/// mutation, no arbitrary JSON traversal into credentials/provider state.
pub fn extract(
    input: &str,
    source: &str,
    session: &str,
    scope: &str,
    repositories: &[String],
    max: usize,
) -> Result<Candidates, String> {
    extract_window(input, source, session, scope, repositories, 0, max)
}

/// Deterministic candidate pagination over one complete immutable input.
pub fn extract_window(
    input: &str,
    source: &str,
    session: &str,
    scope: &str,
    repositories: &[String],
    skip: usize,
    max: usize,
) -> Result<Candidates, String> {
    if [source, session, scope].iter().any(|v| v.trim().is_empty())
        || repositories.is_empty()
        || repositories.iter().any(|r| r.trim().is_empty())
        || !(1..=200).contains(&max)
    {
        return Err(
            "source, session, scope, repositories and max-candidates 1..200 are required".into(),
        );
    }
    let mut repos = repositories.to_vec();
    repos.sort();
    repos.dedup();
    let snapshot = sha256_hex(&[input]);
    let mut result = Candidates {
        version: 1,
        scope: scope.into(),
        snapshot: snapshot.clone(),
        candidates: Vec::new(),
        omitted: 0,
        excluded_derived: 0,
        skipped: 0,
        has_more: false,
    };
    let mut message_ids = std::collections::BTreeSet::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .map_err(|_| format!("invalid JSONL record {}", index + 1))?;
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("record {} lacks a message id", index + 1))?;
        if !message_ids.insert(id.to_owned()) {
            return Err("duplicate message id: reconcile source variants before extraction".into());
        }
        if record
            .get("sessionID")
            .or_else(|| record.get("session_id"))
            .and_then(Value::as_str)
            .is_some_and(|id| id != session)
        {
            return Err("record belongs to a different session".into());
        }
        let kind = record
            .get("type")
            .and_then(Value::as_str)
            .ok_or("record lacks type")?;
        if matches!(kind, "compaction" | "synthetic" | "system") {
            result.excluded_derived += 1;
        }
        for (pointer, speaker, text) in source_texts(&record, kind) {
            let lines: Vec<_> = text.lines().collect();
            for (i, quote) in lines.iter().enumerate() {
                if !eligible(quote) {
                    continue;
                }
                let context = lines[i.saturating_sub(2)..(i + 3).min(lines.len())].join("\n");
                if context.len() > 12_000 {
                    result.omitted += 1;
                    continue;
                }
                if result.skipped < skip {
                    result.skipped += 1;
                    continue;
                }
                if result.candidates.len() == max {
                    result.omitted += 1;
                    result.has_more = true;
                    continue;
                }
                let mut candidate = IntelligenceCandidate {
                    id: String::new(),
                    scope: scope.into(),
                    repositories: repos.clone(),
                    context,
                    evidence: SessionEvidence {
                        source: source.into(),
                        snapshot: snapshot.clone(),
                        session: session.into(),
                        message: id.into(),
                        pointer: pointer.clone(),
                        line: i + 1,
                        quote: (*quote).into(),
                        speaker: speaker.into(),
                        observed_at_ms: record.pointer("/time/created").and_then(Value::as_i64),
                    },
                };
                candidate.id = candidate.identity();
                result.candidates.push(candidate);
            }
        }
    }
    Ok(result)
}

fn source_texts<'a>(record: &'a Value, kind: &str) -> Vec<(String, &'static str, &'a str)> {
    let mut texts = Vec::new();
    if kind == "user" {
        if let Some(text) = record.get("text").and_then(Value::as_str) {
            texts.push(("/text".into(), "user", text));
        }
    } else if kind == "assistant" {
        if let Some(parts) = record.get("content").and_then(Value::as_array) {
            for (i, part) in parts.iter().enumerate() {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            texts.push((format!("/content/{i}/text"), "assistant", text));
                        }
                    }
                    Some("tool") => {
                        if let Some(content) =
                            part.pointer("/state/content").and_then(Value::as_array)
                        {
                            for (j, output) in content.iter().enumerate() {
                                if let Some(text) = output.get("text").and_then(Value::as_str) {
                                    texts.push((
                                        format!("/content/{i}/state/content/{j}/text"),
                                        "tool",
                                        text,
                                    ));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    texts
}

fn eligible(line: &str) -> bool {
    if !(24..=1200).contains(&line.len()) {
        return false;
    }
    let lower = line.to_lowercase();
    // ponytail: English lexical proposals only, not semantic extraction.
    // Measure missed valuable examples before replacing this cheap filter.
    [
        "decision",
        "constraint",
        "must",
        "never",
        "avoid",
        "because",
        "root cause",
        "failed",
        "failure",
        "verified",
        "require",
        "do not",
        "cannot",
        "should",
        "fix",
        "prefer",
        "race",
        "preserve",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}
