//! Responder trait with the deterministic fixture and live Jev responders.

use super::{
    BTreeMap, Question, SystemOneResponse, Answer, ChoiceAnswer, NoulAnswer, ScoreAnswer,
    JevClient, BTreeSet,
};

/// Responder abstraction: production uses the real Jev client; tests use a
/// local protocol fixture (no creds, no network).
#[async_trait::async_trait]
pub trait Responder: Send + Sync {
    /// Answer one batch of questions; production uses the Jev HTTP client.
    async fn respond(
        &mut self,
        state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String>;
}

/// Deterministic fixture responder for tests and `test-typedb`.
pub struct FixtureResponder {
    /// When true every Choice answer is `accept`; otherwise `none`.
    pub accept_all: bool,
    /// Reported model identity (defaults to the pinned Jev model).
    pub model: String,
}

impl FixtureResponder {
    /// A deterministic responder for tests and credential-free runs.
    #[must_use]
    pub fn new(accept_all: bool) -> Self {
        Self {
            accept_all,
            model: chaosbox_jev::JEV_MODEL_PINNED.into(),
        }
    }
}

#[async_trait::async_trait]
impl Responder for FixtureResponder {
    async fn respond(
        &mut self,
        _state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        let mut answers = BTreeMap::new();
        for (id, q) in &questions {
            match q {
                Question::Choice { .. } => {
                    let choice = if self.accept_all { "accept" } else { "none" };
                    answers.insert(
                        id.clone(),
                        Answer::Choice(ChoiceAnswer {
                            choice: choice.into(),
                            probabilities: BTreeMap::from([
                                ("accept".into(), if self.accept_all { 0.9 } else { 0.1 }),
                                ("reject".into(), 0.05),
                                ("none".into(), if self.accept_all { 0.05 } else { 0.85 }),
                            ]),
                            confidence: 0.85,
                        }),
                    );
                }
                Question::Noul { .. } => {
                    answers.insert(id.clone(), Answer::Noul(NoulAnswer { noul: 0.8 }));
                }
                Question::Score { .. } => {
                    answers.insert(
                        id.clone(),
                        Answer::Score(ScoreAnswer {
                            score: 1.0,
                            probabilities: BTreeMap::from([("0".into(), 0.1), ("1".into(), 0.9)]),
                            confidence: 0.8,
                            results: vec![0.1, 0.9],
                        }),
                    );
                }
            }
        }
        Ok(SystemOneResponse {
            model: self.model.clone(),
            answers,
            usage: chaosbox_jev::Usage {
                input_tokens: 100,
                output_tokens: 0,
            },
        })
    }
}

/// Live responder: drives the real [`JevClient`] HTTP adapter.
/// Valid options come from each question's own criteria keys, so candidate
/// membership is enforced on live answers exactly as in tests.
pub struct LiveResponder {
    client: JevClient,
}

impl LiveResponder {
    /// Wrap a configured client (endpoint, model, budgets, retries).
    #[must_use]
    pub fn new(client: JevClient) -> Self {
        Self { client }
    }

    /// Spend for this run as `(requests dispatched, input tokens)`. Every
    /// dispatch counts, including retries, because callers budget a whole
    /// batch against what each run actually sent.
    #[must_use]
    pub fn usage(&self) -> (u32, u64) {
        (self.client.sent_requests(), self.client.spent_tokens())
    }
}

#[async_trait::async_trait]
impl Responder for LiveResponder {
    async fn respond(
        &mut self,
        state: serde_json::Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        let valid: BTreeMap<String, BTreeSet<String>> = questions
            .iter()
            .map(|(id, q)| match q {
                Question::Choice { criteria, .. } => {
                    (id.clone(), criteria.keys().cloned().collect())
                }
                Question::Noul { .. } | Question::Score { .. } => (id.clone(), BTreeSet::new()),
            })
            .collect();
        self.client
            .evaluate(state, questions, &valid)
            .await
            .map_err(|e| e.to_string())
    }
}
