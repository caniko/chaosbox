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
#[derive(Clone, Debug, Deserialize)]
pub struct SystemOneResponse {
    /// Model identity that actually served the request.
    pub model: String,
    /// Answers keyed by the asked question ids.
    pub answers: BTreeMap<String, Answer>,
    /// Token accounting for budgets.
    pub usage: Usage,
}

/// Token accounting returned with every response.
#[derive(Clone, Debug, Deserialize)]
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
mod tests {
    use super::*;

    #[test]
    fn context_limits_reject_silently_truncatable() {
        let big = "x".repeat(CTX_TOTAL_MAX * 5);
        let mut q = BTreeMap::new();
        q.insert(
            "q".into(),
            Question::Noul {
                instructions: "y?".into(),
                criteria: None,
            },
        );
        assert!(
            check_context_limits(&big, &q).is_err(),
            "never silently truncate"
        );
    }

    #[test]
    fn noul_missing_confidence_ok_but_choice_requires_it() {
        let mut asked = BTreeMap::new();
        asked.insert(
            "a".into(),
            Question::Noul {
                instructions: "y?".into(),
                criteria: None,
            },
        );
        let resp = SystemOneResponse {
            model: JEV_MODEL_PINNED.into(),
            answers: BTreeMap::from([("a".into(), Answer::Noul(NoulAnswer { noul: 0.7 }))]),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 0,
            },
        };
        assert!(validate_response(&resp, &asked, &BTreeMap::new()).is_ok());
    }

    #[test]
    fn choice_rejects_out_of_scope() {
        let mut asked = BTreeMap::new();
        asked.insert(
            "c".into(),
            Question::Choice {
                instructions: "pick".into(),
                criteria: BTreeMap::from([
                    ("yes".into(), None),
                    ("no".into(), None),
                    ("none".into(), None),
                ]),
            },
        );
        let resp = SystemOneResponse {
            model: JEV_MODEL_PINNED.into(),
            answers: BTreeMap::from([(
                "c".into(),
                Answer::Choice(ChoiceAnswer {
                    choice: "invented".into(),
                    probabilities: BTreeMap::from([("invented".into(), 1.0)]),
                    confidence: 0.9,
                }),
            )]),
            usage: Usage {
                input_tokens: 5,
                output_tokens: 0,
            },
        };
        let valid = BTreeMap::from([(
            "c".into(),
            BTreeSet::from(["yes".into(), "no".into(), "none".into()]),
        )]);
        assert!(validate_response(&resp, &asked, &valid).is_err());
    }

    #[test]
    fn malformed_probabilities_rejected() {
        assert!(check_probability(f64::INFINITY).is_err());
        assert!(check_probability(-1.0).is_err());
    }

    #[test]
    fn cache_key_changes_with_inputs() {
        let q = BTreeMap::from([(
            "a".into(),
            Question::Noul {
                instructions: "y?".into(),
                criteria: None,
            },
        )]);
        let k1 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r1");
        let k2 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r2");
        assert_ne!(k1, k2);
    }

    #[test]
    fn answer_wire_shape_round_trips() {
        // The "type" discriminant appears exactly once; variant structs must
        // not carry their own copy (serde consumes the tag before decoding).
        let a = Answer::Choice(ChoiceAnswer {
            choice: "accept".into(),
            probabilities: BTreeMap::from([("accept".into(), 1.0)]),
            confidence: 0.9,
        });
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s.matches("\"type\"").count(), 1, "{s}");
        let back: Answer = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, Answer::Choice(_)));
    }

    #[test]
    fn no_ambient_provider_fallback() {
        for v in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GOOGLE_API_KEY",
            "OLLAMA_HOST",
        ] {
            assert!(!format!("{:?}", JevClient::api_key()).contains(v));
        }
    }
}

#[cfg(test)]
// The mock server holds test-only env-serialization locks across awaits by
// design (no production deadlock surface); allowed to keep the wire tests
// readable. Production paths never hold a guard across await.
#[allow(clippy::await_holding_lock)]
mod http_tests {
    //! Wire-level tests for [`super::JevClient::evaluate`] against a scripted
    //! mock `POST /v1/systemone` server on 127.0.0.1 (raw `tokio` TCP, no new
    //! dependencies, no credentials, no network beyond loopback).
    use super::*;
    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex, OnceLock};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Process-global credential env is shared by threads: serialize the
    /// wire tests so each sees its own test key.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// One scripted HTTP response.
    struct Script {
        status: u16,
        retry_after: Option<u64>,
        body: String,
    }

    fn find_crlf2(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
    }

    fn content_len(head: &[u8]) -> usize {
        let s = String::from_utf8_lossy(head).to_lowercase();
        s.lines()
            .find_map(|l| {
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0)
    }

    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            401 => "Unauthorized",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Error",
        }
    }

    /// Serve the scripts in order; records raw request heads; returns the
    /// endpoint URL and the join handle resolving to requests served.
    async fn serve(
        scripts: Vec<Script>,
        heads: Arc<Mutex<Vec<String>>>,
    ) -> (String, tokio::task::JoinHandle<usize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let mut served = 0usize;
            for s in scripts {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while let Ok(n) = sock.read(&mut tmp).await {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(h) = find_crlf2(&buf) {
                        if buf.len() >= h + content_len(&buf[..h]) {
                            heads
                                .lock()
                                .unwrap()
                                .push(String::from_utf8_lossy(&buf[..h]).into_owned());
                            break;
                        }
                    }
                    if buf.len() > 4_000_000 {
                        break;
                    }
                }
                let mut resp = format!(
                    "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                    s.status,
                    reason(s.status),
                    s.body.len()
                );
                if let Some(ra) = s.retry_after {
                    let _ = write!(resp, "retry-after: {ra}\r\n");
                }
                resp.push_str("\r\n");
                resp.push_str(&s.body);
                if sock.write_all(resp.as_bytes()).await.is_err() {
                    break;
                }
                served += 1;
            }
            served
        });
        (format!("http://{addr}/v1/systemone"), h)
    }

    fn test_policy(endpoint: String) -> JevPolicy {
        JevPolicy {
            endpoint,
            deadline: Duration::from_secs(10),
            max_questions_per_request: 4,
            max_retries: 3,
            ..JevPolicy::default()
        }
    }

    fn choice_questions() -> BTreeMap<String, Question> {
        BTreeMap::from([(
            "q1".to_owned(),
            Question::Choice {
                instructions: "pick one".to_owned(),
                criteria: BTreeMap::from([
                    ("accept".to_owned(), None),
                    ("reject".to_owned(), None),
                    ("none".to_owned(), None),
                ]),
            },
        )])
    }

    fn valid_options() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([(
            "q1".to_owned(),
            BTreeSet::from(["accept".to_owned(), "reject".to_owned(), "none".to_owned()]),
        )])
    }

    fn accept_body() -> String {
        serde_json::json!({
            "model": JEV_MODEL_PINNED,
            "answers": {"q1": {
                "type": "choice", "choice": "accept",
                "probabilities": {"accept": 0.9, "reject": 0.05, "none": 0.05},
                "confidence": 0.85,
            }},
            "usage": {"input_tokens": 10, "output_tokens": 0},
        })
        .to_string()
    }

    fn use_test_key(name: &str) {
        std::env::remove_var("CHAOSBOX_JEV_API_KEY_FILE");
        std::env::set_var("TYPESAFE_API_KEY", format!("test-key-{name}"));
    }

    #[tokio::test]
    async fn retry_after_honored_then_success_over_http() {
        let _guard = env_lock().lock().unwrap();
        use_test_key("retry");
        let heads = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            vec![
                Script {
                    status: 429,
                    retry_after: Some(0),
                    body: "{}".to_owned(),
                },
                Script {
                    status: 200,
                    retry_after: None,
                    body: accept_body(),
                },
            ],
            heads.clone(),
        )
        .await;
        let mut client = JevClient::new(test_policy(url)).unwrap();
        let resp = client
            .evaluate(
                serde_json::json!({"repo": "demo"}),
                choice_questions(),
                &valid_options(),
            )
            .await
            .unwrap();
        assert_eq!(resp.model, JEV_MODEL_PINNED);
        assert_eq!(client.attempts.len(), 2);
        assert_eq!(client.attempts[0].http_status, Some(429));
        assert_eq!(client.attempts[0].retry_after_secs, Some(0));
        assert_eq!(client.attempts[1].http_status, Some(200));
        // Bearer auth on the wire, never a query param or log line.
        let heads = heads.lock().unwrap();
        assert_eq!(heads.len(), 2);
        assert!(heads[0].contains("authorization: Bearer test-key-retry"));
        assert!(!heads[0].contains("test-key-retry\""));
        assert_eq!(server.await.unwrap(), 2);
    }

    #[tokio::test]
    async fn auth_failure_never_retried() {
        let _guard = env_lock().lock().unwrap();
        use_test_key("auth");
        let heads = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            vec![Script {
                status: 401,
                retry_after: None,
                body: "{}".to_owned(),
            }],
            heads,
        )
        .await;
        let mut client = JevClient::new(test_policy(url)).unwrap();
        let err = client
            .evaluate(serde_json::json!({}), choice_questions(), &valid_options())
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Auth(_)), "got {err:?}");
        assert_eq!(server.await.unwrap(), 1, "auth errors must not be retried");
    }

    #[tokio::test]
    async fn out_of_scope_choice_rejected_over_http() {
        let _guard = env_lock().lock().unwrap();
        use_test_key("scope");
        let body = serde_json::json!({
            "model": JEV_MODEL_PINNED,
            "answers": {"q1": {
                "type": "choice", "choice": "invented",
                "probabilities": {"invented": 1.0},
                "confidence": 0.9,
            }},
            "usage": {"input_tokens": 5, "output_tokens": 0},
        })
        .to_string();
        let (url, server) = serve(
            vec![Script {
                status: 200,
                retry_after: None,
                body,
            }],
            Arc::new(Mutex::new(Vec::new())),
        )
        .await;
        let mut client = JevClient::new(test_policy(url)).unwrap();
        let err = client
            .evaluate(serde_json::json!({}), choice_questions(), &valid_options())
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Schema(_)), "got {err:?}");
        assert_eq!(server.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn missing_answer_is_protocol_error() {
        let _guard = env_lock().lock().unwrap();
        use_test_key("missing");
        let body = serde_json::json!({
            "model": JEV_MODEL_PINNED,
            "answers": {},
            "usage": {"input_tokens": 5, "output_tokens": 0},
        })
        .to_string();
        let (url, server) = serve(
            vec![Script {
                status: 200,
                retry_after: None,
                body,
            }],
            Arc::new(Mutex::new(Vec::new())),
        )
        .await;
        let mut client = JevClient::new(test_policy(url)).unwrap();
        let err = client
            .evaluate(serde_json::json!({}), choice_questions(), &valid_options())
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Protocol(_)), "got {err:?}");
        assert_eq!(server.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn malformed_probability_rejected_over_http() {
        let _guard = env_lock().lock().unwrap();
        use_test_key("badprob");
        let body = serde_json::json!({
            "model": JEV_MODEL_PINNED,
            "answers": {"q1": {"type": "noul", "noul": 7.5}},
            "usage": {"input_tokens": 5, "output_tokens": 0},
        })
        .to_string();
        let questions = BTreeMap::from([(
            "q1".to_owned(),
            Question::Noul {
                instructions: "y?".to_owned(),
                criteria: None,
            },
        )]);
        let (url, server) = serve(
            vec![Script {
                status: 200,
                retry_after: None,
                body,
            }],
            Arc::new(Mutex::new(Vec::new())),
        )
        .await;
        let mut client = JevClient::new(test_policy(url)).unwrap();
        let err = client
            .evaluate(serde_json::json!({}), questions, &BTreeMap::new())
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Schema(_)), "got {err:?}");
        assert_eq!(server.await.unwrap(), 1);
    }
}
