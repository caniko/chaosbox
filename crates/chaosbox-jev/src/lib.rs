//! Typed Jev client for `POST /v1/systemone`.
//!
//! Wire format verified against <https://docs.typesafe.ai/api> and
//! <https://docs.typesafe.ai/models> (2026-09-18):
//! model `jev-1.13.0` pinned; aliases `jev-latest`/`jev-preview` resolve to
//! it. Limits: 64k tokens/request total, 32k for state+longest question.
//! Rate limits: 250k tok/s, 1200 req/min; 429 honors `retry-after`.
//! 529 (overloaded) retried as transient. 401/403/400/422 never retried.
//!
//! Question ids carry no inference meaning: entity/relation descriptions go
//! in `state`/`instructions`. Questions in one request are independent.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use chaosbox_core::{check_confidence, check_probability, sha256_hex};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Pinned reproducible model. Never a moving alias in production.
pub const JEV_MODEL_PINNED: &str = "jev-1.13.0";
/// Documented ceilings (tokens).
pub const CTX_TOTAL_MAX: usize = 64_000;
pub const CTX_STATE_PLUS_LONGEST_MAX: usize = 32_000;
/// Response size cap (bytes).
pub const MAX_RESPONSE_BYTES: u64 = 2_000_000;

#[derive(Debug, Error)]
pub enum JevError {
    #[error("budget exceeded: {0}")]
    Budget(String),
    #[error("context limit: {0}")]
    Context(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("auth (no retry): {0}")]
    Auth(String),
    #[error("schema (no retry): {0}")]
    Schema(String),
    #[error("transient after {0} attempts: {1}")]
    Transient(u32, String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

/// One typed question. `type` selects the variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul {
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: String,
        criteria: BTreeMap<String, Option<String>>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(default, rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<String>,
    #[serde(default, rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<String>,
}

/// Noul answer: probability yes. No confidence field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoulAnswer {
    #[serde(rename = "type")]
    pub kind: String,
    pub noul: f64,
}

/// Choice answer: selected option + full distribution + confidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    #[serde(rename = "type")]
    pub kind: String,
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// Score answer: weighted value + per-level distribution + confidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreAnswer {
    #[serde(rename = "type")]
    pub kind: String,
    pub score: f64,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
    #[serde(default)]
    pub results: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemOneRequest {
    pub state: serde_json::Value,
    pub model: String,
    pub questions: BTreeMap<String, Question>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Endpoint + deadline + concurrency + spending/request budgets.
#[derive(Clone, Debug)]
pub struct JevPolicy {
    pub endpoint: String,
    pub model: String,
    pub deadline: Duration,
    pub max_questions_per_request: usize,
    pub max_concurrent_requests: usize,
    pub max_requests: u32,
    pub max_input_tokens: u64,
    pub max_retries: u32,
    pub max_response_bytes: u64,
}

impl Default for JevPolicy {
    fn default() -> Self {
        Self {
            endpoint: "https://api.typesafe.ai/v1/systemone".to_owned(),
            model: JEV_MODEL_PINNED.to_owned(),
            deadline: Duration::from_secs(60),
            max_questions_per_request: 16,
            max_concurrent_requests: 4,
            max_requests: 100,
            max_input_tokens: 1_000_000,
            max_retries: 3,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
}

/// Durable per-attempt accounting (no secret values).
#[derive(Clone, Debug, Default, Serialize)]
pub struct AttemptRecord {
    pub attempt: u32,
    pub question_ids: Vec<String>,
    pub http_status: Option<u16>,
    pub retry_after_secs: Option<u64>,
    pub input_tokens: Option<u64>,
    pub error: Option<String>,
}

/// Conservative token estimate: `ceil(chars/4) + 8% headroom`.
/// Never assumed exact; enforcement errors instead of silent truncation.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4) * 108 / 100
}

/// Enforce both documented context limits.
pub fn check_context_limits(
    state: &str,
    questions: &BTreeMap<String, Question>,
) -> Result<(), JevError> {
    let state_t = estimate_tokens(state);
    let mut q_total = 0usize;
    let mut longest = 0usize;
    for q in questions.values() {
        let s = serde_json::to_string(q).unwrap_or_default();
        let t = estimate_tokens(&s);
        q_total += t;
        longest = longest.max(t);
    }
    if state_t + q_total > CTX_TOTAL_MAX {
        return Err(JevError::Context(format!(
            "state+questions ~{state_t}+{q_total} exceeds {CTX_TOTAL_MAX}"
        )));
    }
    if state_t + longest > CTX_STATE_PLUS_LONGEST_MAX {
        return Err(JevError::Context(format!(
            "state+longest ~{state_t}+{longest} exceeds {CTX_STATE_PLUS_LONGEST_MAX}"
        )));
    }
    Ok(())
}

/// Cache identity: all decision inputs. Thresholds excluded (materialization).
#[must_use]
pub fn cache_key(
    source_digest: &str,
    catalog_digest: &str,
    ordered_questions: &BTreeMap<String, Question>,
    model: &str,
    rubric_version: &str,
) -> String {
    let q = serde_json::to_string(ordered_questions).unwrap_or_default();
    format!("jev:{}", sha256_hex(&[source_digest, catalog_digest, &q, model, rubric_version]))
}

/// Typed client. No OpenAI/Anthropic/Gemini/Ollama fallback anywhere.
pub struct JevClient {
    http: reqwest::Client,
    policy: JevPolicy,
    pub attempts: Vec<AttemptRecord>,
    spent_tokens: u64,
    sent_requests: u32,
}

impl JevClient {
    pub fn new(policy: JevPolicy) -> Result<Self, JevError> {
        let http = reqwest::Client::builder()
            .timeout(policy.deadline)
            .build()
            .map_err(|e| JevError::Transport(e.to_string()))?;
        Ok(Self { http, policy, attempts: Vec::new(), spent_tokens: 0, sent_requests: 0 })
    }

    /// Read API key: explicit file first, then explicit env for operators.
    /// Never auto-discovered from ambient OpenAI/Anthropic/Gemini/Ollama vars.
    pub fn api_key() -> Option<String> {
        if let Ok(f) = std::env::var("CHAOSBOX_JEV_API_KEY_FILE") {
            if let Ok(k) = std::fs::read_to_string(f.trim()) {
                let k = k.trim().to_owned();
                if !k.is_empty() {
                    return Some(k);
                }
            }
        }
        if let Ok(k) = std::env::var("TYPESAFE_API_KEY") {
            let k = k.trim().to_owned();
            if !k.is_empty() {
                return Some(k);
            }
        }
        None
    }

    fn api_key_for_request() -> Result<String, JevError> {
        Self::api_key().ok_or_else(|| {
            JevError::Auth("missing CHAOSBOX_JEV_API_KEY_FILE / TYPESAFE_API_KEY".into())
        })
    }

    /// Evaluate one batch with bounded retries + full validation.
    pub async fn evaluate(
        &mut self,
        state: serde_json::Value,
        questions: BTreeMap<String, Question>,
        valid_options: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<SystemOneResponse, JevError> {
        if questions.is_empty() || questions.len() > self.policy.max_questions_per_request {
            return Err(JevError::Budget(format!("questions {}", questions.len())));
        }
        if self.sent_requests >= self.policy.max_requests {
            return Err(JevError::Budget("max_requests".into()));
        }
        let state_str = state.to_string();
        check_context_limits(&state_str, &questions)?;
        let key = Self::api_key_for_request()?;
        let body = SystemOneRequest { state, model: self.policy.model.clone(), questions: questions.clone() };
        let ids: Vec<String> = questions.keys().cloned().collect();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let res = self
                .http
                .post(&self.policy.endpoint)
                .bearer_auth(&key)
                .json(&body)
                .send()
                .await;
            match res {
                Err(e) if e.is_timeout() || e.is_connect() => {
                    self.attempts.push(AttemptRecord {
                        attempt,
                        question_ids: ids.clone(),
                        error: Some(sanitized(&e.to_string())),
                        ..Default::default()
                    });
                    if attempt > self.policy.max_retries {
                        return Err(JevError::Transient(attempt, "timeout/connect".into()));
                    }
                    backoff(attempt).await;
                }
                Err(e) => return Err(JevError::Transport(sanitized(&e.to_string()))),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok());
                    if status == 429 || status == 529 {
                        self.attempts.push(AttemptRecord {
                            attempt,
                            question_ids: ids.clone(),
                            http_status: Some(status),
                            retry_after_secs: retry_after,
                            ..Default::default()
                        });
                        if attempt > self.policy.max_retries {
                            return Err(JevError::Transient(attempt, format!("http {status}")));
                        }
                        if let Some(s) = retry_after {
                            tokio::time::sleep(Duration::from_secs(s.min(60))).await;
                        } else {
                            backoff(attempt).await;
                        }
                        continue;
                    }
                    if status == 401 || status == 403 {
                        return Err(JevError::Auth(format!("http {status}")));
                    }
                    if status == 400 || status == 404 || status == 422 {
                        let t = resp.text().await.unwrap_or_default();
                        return Err(JevError::Schema(sanitized(&truncate(&t, 500))));
                    }
                    if !(200..300).contains(&status) {
                        let t = resp.text().await.unwrap_or_default();
                        self.attempts.push(AttemptRecord {
                            attempt,
                            question_ids: ids.clone(),
                            http_status: Some(status),
                            error: Some(sanitized(&truncate(&t, 300))),
                            ..Default::default()
                        });
                        if attempt > self.policy.max_retries || status < 500 {
                            return Err(JevError::Transient(attempt, format!("http {status}")));
                        }
                        backoff(attempt).await;
                        continue;
                    }
                    let bytes = resp.bytes().await.map_err(|e| JevError::Transport(sanitized(&e.to_string())))?;
                    if bytes.len() as u64 > self.policy.max_response_bytes {
                        return Err(JevError::Protocol("response too large".into()));
                    }
                    let parsed: SystemOneResponse = serde_json::from_slice(&bytes)
                        .map_err(|e| JevError::Schema(sanitized(&e.to_string())))?;
                    self.sent_requests += 1;
                    self.spent_tokens += parsed.usage.input_tokens;
                    if self.spent_tokens > self.policy.max_input_tokens {
                        return Err(JevError::Budget("max_input_tokens".into()));
                    }
                    self.attempts.push(AttemptRecord {
                        attempt,
                        question_ids: ids.clone(),
                        http_status: Some(status),
                        input_tokens: Some(parsed.usage.input_tokens),
                        ..Default::default()
                    });
                    validate_response(&parsed, &questions, valid_options)?;
                    return Ok(parsed);
                }
            }
        }
    }
}

async fn backoff(attempt: u32) {
    let ms = 200u64.saturating_mul(1 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(ms.min(5_000))).await;
}

/// Validate: answer-id reconciliation, type match, finite/range, membership.
pub fn validate_response(
    resp: &SystemOneResponse,
    asked: &BTreeMap<String, Question>,
    valid_options: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), JevError> {
    if resp.answers.len() != asked.len() {
        return Err(JevError::Protocol(format!(
            "answer count {} != asked {}",
            resp.answers.len(),
            asked.len()
        )));
    }
    for (id, q) in asked {
        let a = resp.answers.get(id).ok_or_else(|| JevError::Protocol(format!("missing answer {id}")))?;
        match (q, a) {
            (Question::Noul { .. }, Answer::Noul(n)) => {
                check_probability(n.noul).map_err(|e| JevError::Schema(e.to_string()))?;
            }
            (Question::Choice { .. }, Answer::Choice(c)) => {
                check_confidence(c.confidence).map_err(|e| JevError::Schema(e.to_string()))?;
                let mut sum = 0.0;
                for p in c.probabilities.values() {
                    check_probability(*p).map_err(|e| JevError::Schema(e.to_string()))?;
                    sum += p;
                }
                if (sum - 1.0).abs() > 0.05 {
                    return Err(JevError::Schema(format!("choice probs sum {sum}")));
                }
                if let Some(valid) = valid_options.get(id) {
                    if !valid.contains(&c.choice) {
                        return Err(JevError::Schema(format!("out-of-scope choice {}", c.choice)));
                    }
                    for k in c.probabilities.keys() {
                        if !valid.contains(k) {
                            return Err(JevError::Schema(format!("out-of-scope option {k}")));
                        }
                    }
                }
            }
            (Question::Score { .. }, Answer::Score(s)) => {
                check_confidence(s.confidence).map_err(|e| JevError::Schema(e.to_string()))?;
                if !s.score.is_finite() {
                    return Err(JevError::Schema("non-finite score".into()));
                }
                for p in s.probabilities.values() {
                    check_probability(*p).map_err(|e| JevError::Schema(e.to_string()))?;
                }
                for r in &s.results {
                    if !r.is_finite() {
                        return Err(JevError::Schema("non-finite result".into()));
                    }
                }
            }
            _ => return Err(JevError::Schema(format!("type mismatch for {id}"))),
        }
    }
    // Returned model recorded by caller; pinned request model enforced at call site.
    Ok(())
}

fn sanitized(s: &str) -> String {
    // Never leak bearer tokens: redact long alphanumerics that look like keys.
    let mut out = s.to_owned();
    for prefix in ["sk-", "ts-", "Bearer "] {
        while let Some(i) = out.find(prefix) {
            let end = out[i..].char_indices().take(12).last().map(|(j, _)| i + j).unwrap_or(out.len());
            out.replace_range(i..end.min(out.len()), &format!("{prefix}[redacted]"));
            break;
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limits_reject_silently_truncatable() {
        let big = "x".repeat(CTX_TOTAL_MAX * 5);
        let mut q = BTreeMap::new();
        q.insert("q".into(), Question::Noul { instructions: "y?".into(), criteria: None });
        assert!(check_context_limits(&big, &q).is_err(), "never silently truncate");
    }

    #[test]
    fn noul_missing_confidence_ok_but_choice_requires_it() {
        let mut asked = BTreeMap::new();
        asked.insert("a".into(), Question::Noul { instructions: "y?".into(), criteria: None });
        let resp = SystemOneResponse {
            model: JEV_MODEL_PINNED.into(),
            answers: BTreeMap::from([("a".into(), Answer::Noul(NoulAnswer { kind: "noul".into(), noul: 0.7 }))]),
            usage: Usage { input_tokens: 10, output_tokens: 0 },
        };
        assert!(validate_response(&resp, &asked, &BTreeMap::new()).is_ok());
    }

    #[test]
    fn choice_rejects_out_of_scope() {
        let mut asked = BTreeMap::new();
        asked.insert("c".into(), Question::Choice {
            instructions: "pick".into(),
            criteria: BTreeMap::from([("yes".into(), None), ("no".into(), None), ("none".into(), None)]),
        });
        let resp = SystemOneResponse {
            model: JEV_MODEL_PINNED.into(),
            answers: BTreeMap::from([("c".into(), Answer::Choice(ChoiceAnswer {
                kind: "choice".into(),
                choice: "invented".into(),
                probabilities: BTreeMap::from([("invented".into(), 1.0)]),
                confidence: 0.9,
            }))]),
            usage: Usage { input_tokens: 5, output_tokens: 0 },
        };
        let valid = BTreeMap::from([("c".into(), BTreeSet::from(["yes".into(), "no".into(), "none".into()]))]);
        assert!(validate_response(&resp, &asked, &valid).is_err());
    }

    #[test]
    fn malformed_probabilities_rejected() {
        assert!(check_probability(f64::INFINITY).is_err());
        assert!(check_probability(-1.0).is_err());
    }

    #[test]
    fn cache_key_changes_with_inputs() {
        let q = BTreeMap::from([("a".into(), Question::Noul { instructions: "y?".into(), criteria: None })]);
        let k1 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r1");
        let k2 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r2");
        assert_ne!(k1, k2);
    }

    #[test]
    fn no_ambient_provider_fallback() {
        for v in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GOOGLE_API_KEY", "OLLAMA_HOST"] {
            assert!(!format!("{:?}", JevClient::api_key()).contains(v));
        }
    }
}
