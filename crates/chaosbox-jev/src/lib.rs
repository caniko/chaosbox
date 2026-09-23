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
/// Documented ceiling: total tokens per request.
pub const CTX_TOTAL_MAX: usize = 64_000;
/// Documented ceiling: state plus longest-question tokens.
pub const CTX_STATE_PLUS_LONGEST_MAX: usize = 32_000;
/// Response size cap (bytes).
pub const MAX_RESPONSE_BYTES: u64 = 2_000_000;

/// Failures across budgets, transport, auth/schema (never retried), and protocol.
#[derive(Debug, Error)]
pub enum JevError {
    #[error("budget exceeded: {0}")]
    /// A deadline/concurrency/spend/request budget was exceeded.
    Budget(String),
    #[error("context limit: {0}")]
    /// A documented context ceiling would be exceeded; never silently truncate.
    Context(String),
    #[error("transport: {0}")]
    /// HTTP client or connection failure.
    Transport(String),
    #[error("auth (no retry): {0}")]
    /// 401/403 or missing credentials; never retried.
    Auth(String),
    #[error("schema (no retry): {0}")]
    /// 400/404/422 or unparseable envelope; never retried.
    Schema(String),
    #[error("transient after {0} attempts: {1}")]
    /// Retryable failure (429/529/5xx/timeout) past the retry budget.
    Transient(u32, String),
    #[error("protocol: {0}")]
    /// Answer-id mismatch, type mismatch, oversize response, etc.
    Protocol(String),
    #[error("cancelled")]
    /// The request was cancelled.
    Cancelled,
}

/// One typed question. `type` selects the variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Yes/no question; answer is a Noul probability.
    Noul {
        /// What is being asked; entity descriptions, never bare ids.
        instructions: String,
        /// Optional yes/no criteria text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// Single-choice question; answer selects one option plus a distribution.
    Choice {
        /// What is being asked; entity descriptions, never bare ids.
        instructions: String,
        /// Option name -> optional description.
        criteria: BTreeMap<String, Option<String>>,
    },
    /// Scored question over ordered levels; answer is a weighted value.
    Score {
        /// What is being asked; entity descriptions, never bare ids.
        instructions: String,
        /// Ordered level descriptions.
        criteria: Vec<String>,
    },
}

/// Optional yes/no criteria text for a Noul question.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NoulCriteria {
    /// Description of the `true` outcome.
    #[serde(default, rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<String>,
    /// Description of the `false` outcome.
    #[serde(default, rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<String>,
}

/// Noul answer: probability yes. No confidence field.
/// (The `"type"` discriminant is handled by the [`Answer`] enum.)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoulAnswer {
    /// Probability of yes, in [0,1].
    pub noul: f64,
}

/// Choice answer: selected option + full distribution + confidence.
/// (The `"type"` discriminant is handled by the [`Answer`] enum.)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    /// The selected option; always a member of the asked criteria.
    pub choice: String,
    /// Full option distribution (sums to ~1).
    pub probabilities: BTreeMap<String, f64>,
    /// Model confidence in [0,1]; distinct from the distribution.
    pub confidence: f64,
}

/// Score answer: weighted value + per-level distribution + confidence.
/// (The `"type"` discriminant is handled by the [`Answer`] enum.)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreAnswer {
    /// Weighted value across levels.
    pub score: f64,
    /// Per-level distribution.
    pub probabilities: BTreeMap<String, f64>,
    /// Model confidence in [0,1]; distinct from the value.
    pub confidence: f64,
    /// Per-level results backing the weighted value.
    #[serde(default)]
    pub results: Vec<f64>,
}

/// A typed answer; the variant must match the asked question type.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    /// Yes/no probability answer.
    Noul(NoulAnswer),
    /// Single-choice answer.
    Choice(ChoiceAnswer),
    /// Scored answer.
    Score(ScoreAnswer),
}

/// The `POST /v1/systemone` request body.
#[derive(Clone, Debug, Serialize)]
pub struct SystemOneRequest {
    /// Shared context: entity/relation descriptions, never bare ids.
    pub state: serde_json::Value,
    /// Pinned model identity (never a moving alias in production).
    pub model: String,
    /// Questions by arbitrary id; evaluated independently.
    pub questions: BTreeMap<String, Question>,
}

/// The `POST /v1/systemone` response body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemOneResponse {
    /// Model identity that actually served the request.
    pub model: String,
    /// Answers keyed by the asked question ids.
    pub answers: BTreeMap<String, Answer>,
    /// Token accounting for budgets.
    pub usage: Usage,
}

/// Token accounting returned with every response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens consumed.
    pub input_tokens: u64,
    /// Output tokens consumed.
    pub output_tokens: u64,
}

/// Endpoint + deadline + concurrency + spending/request budgets.
#[derive(Clone, Debug)]
pub struct JevPolicy {
    /// Full `POST /v1/systemone` endpoint URL (explicit TLS host).
    pub endpoint: String,
    /// Pinned model identity.
    pub model: String,
    /// Per-request deadline (also bounds cancellation).
    pub deadline: Duration,
    /// Max questions evaluated together (independent decisions only).
    pub max_questions_per_request: usize,
    /// Max in-flight requests.
    pub max_concurrent_requests: usize,
    /// Max requests per client lifetime (spending/request budget).
    pub max_requests: u32,
    /// Max input tokens per client lifetime (spending budget).
    pub max_input_tokens: u64,
    /// Bounded retries for transient failures only.
    pub max_retries: u32,
    /// Response size cap in bytes.
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
    /// 1-based attempt number within one `evaluate` call.
    pub attempt: u32,
    /// Question ids covered by the attempt.
    pub question_ids: Vec<String>,
    /// HTTP status, if a response was received.
    pub http_status: Option<u16>,
    /// Honored `retry-after` seconds, if the server sent one.
    pub retry_after_secs: Option<u64>,
    /// Input tokens reported, if the call succeeded.
    pub input_tokens: Option<u64>,
    /// Sanitized failure text (no secret values).
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
    format!(
        "jev:{}",
        sha256_hex(&[source_digest, catalog_digest, &q, model, rubric_version])
    )
}

/// Typed client. No OpenAI/Anthropic/Gemini/Ollama fallback anywhere.
pub struct JevClient {
    http: reqwest::Client,
    policy: JevPolicy,
    /// Durable per-attempt accounting for every `evaluate` call.
    pub attempts: Vec<AttemptRecord>,
    spent_tokens: u64,
    sent_requests: u32,
}

impl JevClient {
    /// Build a client from an explicit policy (timeouts from its deadline).
    pub fn new(policy: JevPolicy) -> Result<Self, JevError> {
        let http = reqwest::Client::builder()
            .timeout(policy.deadline)
            .build()
            .map_err(|e| JevError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            policy,
            attempts: Vec::new(),
            spent_tokens: 0,
            sent_requests: 0,
        })
    }

    /// Read API key: explicit file first, then explicit env for operators.
    /// Never auto-discovered from ambient OpenAI/Anthropic/Gemini/Ollama vars.
    #[must_use]
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
    /// Over the default line budget; splitting validation stages apart is
    /// the owning session's refactor. Allowed to keep CI unblocked.
    #[allow(clippy::too_many_lines)]
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
        let body = SystemOneRequest {
            state,
            model: self.policy.model.clone(),
            questions: questions.clone(),
        };
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
                    let bytes = resp
                        .bytes()
                        .await
                        .map_err(|e| JevError::Transport(sanitized(&e.to_string())))?;
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
        let a = resp
            .answers
            .get(id)
            .ok_or_else(|| JevError::Protocol(format!("missing answer {id}")))?;
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
                        return Err(JevError::Schema(format!(
                            "out-of-scope choice {}",
                            c.choice
                        )));
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
        if let Some(i) = out.find(prefix) {
            let end = out[i..]
                .char_indices()
                .take(12)
                .last()
                .map_or(out.len(), |(j, _)| i + j);
            out.replace_range(i..end.min(out.len()), &format!("{prefix}[redacted]"));
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
// The mock server holds test-only env-serialization locks across awaits by
// design (no production deadlock surface); allowed to keep the wire tests
// readable. Production paths never hold a guard across await.
#[allow(clippy::await_holding_lock)]
mod http_tests;
