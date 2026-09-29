//! Bounded operator bridge to the shared Jev client for research scripts.

use std::{collections::BTreeMap, fs::File, io::Read, path::PathBuf};

use chaosbox_jev::{
    Answer, JevClient, JevError, JevPolicy, Question, SystemOneResponse, JEV_MODEL_PINNED,
};
use clap::Subcommand;
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_BYTES: u64 = 200_000;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Evaluate up to 16 independent Choice questions using pinned Typesafe Jev.
    /// Emits a JSON receipt on success or failure. One attempt, no fallback.
    Evaluate {
        /// JSON containing model, state and questions (the System One shape).
        #[arg(long)]
        input: PathBuf,
        /// Explicit consent to send this input to Typesafe.
        #[arg(long, required = true)]
        privacy_reviewed: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    model: String,
    state: Value,
    questions: BTreeMap<String, Question>,
}

fn read_input(path: &PathBuf) -> Option<Input> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let input: Input = serde_json::from_slice(&bytes).ok()?;
    if input.model != JEV_MODEL_PINNED
        || !matches!(
            input.state,
            Value::String(_) | Value::Object(_) | Value::Array(_)
        )
        || input.questions.is_empty()
        || input.questions.len() > 16
        || input.questions.iter().any(|(id, question)| {
            id.is_empty()
                || match question {
                    Question::Choice {
                        instructions,
                        criteria,
                    } => {
                        instructions.trim().is_empty()
                            || !(2..=255).contains(&criteria.len())
                            || criteria.keys().any(|key| key.trim().is_empty())
                    }
                    _ => true,
                }
        })
    {
        return None;
    }
    Some(input)
}

fn error_kind(error: &JevError) -> &'static str {
    // Never print provider error bodies: they can contain source text or keys.
    match error {
        JevError::Budget(_) => "budget",
        JevError::Context(_) => "context",
        JevError::Transport(_) => "transport",
        JevError::Auth(_) => "auth",
        JevError::Schema(_) => "schema",
        JevError::Transient(..) => "transient",
        JevError::Protocol(_) => "protocol",
        JevError::Cancelled => "cancelled",
    }
}

fn complete_choices(response: &SystemOneResponse, questions: &BTreeMap<String, Question>) -> bool {
    questions.iter().all(|(id, question)| {
        let (Question::Choice { criteria, .. }, Some(Answer::Choice(answer))) =
            (question, response.answers.get(id))
        else {
            return false;
        };
        answer.probabilities.keys().eq(criteria.keys())
            && (answer.probabilities.values().sum::<f64>() - 1.0).abs() <= 0.01
            && answer
                .probabilities
                .get(&answer.choice)
                .is_some_and(|selected| {
                    answer.probabilities.values().all(|value| value <= selected)
                })
    })
}

pub async fn run(command: Command) -> (Value, bool) {
    let Command::Evaluate {
        input,
        privacy_reviewed,
    } = command;
    let mut receipt = json!({"version":1, "model_requested":JEV_MODEL_PINNED,
        "sent_requests":0, "input_tokens":0, "response":null, "error":null});
    let Some(input) = read_input(&input).filter(|_| privacy_reviewed) else {
        receipt["error"] = json!("invalid_request");
        return (receipt, false);
    };
    let valid_options = input
        .questions
        .iter()
        .filter_map(|(id, question)| {
            if let Question::Choice { criteria, .. } = question {
                Some((id.clone(), criteria.keys().cloned().collect()))
            } else {
                None
            }
        })
        .collect();
    let policy = JevPolicy {
        max_requests: 1,
        max_retries: 0,
        max_input_tokens: MAX_BYTES,
        ..JevPolicy::default()
    };
    let mut client = match JevClient::new(policy) {
        Ok(client) => client,
        Err(error) => {
            receipt["error"] = json!(error_kind(&error));
            return (receipt, false);
        }
    };
    let result = client
        .evaluate(input.state, input.questions.clone(), &valid_options)
        .await;
    receipt["sent_requests"] = json!(client.sent_requests());
    receipt["input_tokens"] = json!(client.spent_tokens());
    match result {
        Ok(response)
            if response.model == JEV_MODEL_PINNED
                && complete_choices(&response, &input.questions) =>
        {
            receipt["response"] = json!(response);
            (receipt, true)
        }
        Ok(_) => {
            receipt["error"] = json!("protocol");
            (receipt, false)
        }
        Err(error) => {
            receipt["error"] = json!(error_kind(&error));
            (receipt, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributions_must_be_complete_and_select_a_maximum() {
        let questions = serde_json::from_value(json!({"q":{"type":"choice",
            "instructions":"Choose", "criteria":{"yes":"Yes", "no":"No"}}}))
        .unwrap();
        let mut response: SystemOneResponse = serde_json::from_value(json!({
            "model":JEV_MODEL_PINNED, "usage":{"input_tokens":1,"output_tokens":1},
            "answers":{"q":{"type":"choice", "choice":"yes", "confidence":1.0,
            "probabilities":{"yes":1.0,"no":0.0}}}}))
        .unwrap();
        assert!(complete_choices(&response, &questions));
        if let Some(Answer::Choice(answer)) = response.answers.get_mut("q") {
            answer.probabilities.remove("no");
        }
        assert!(!complete_choices(&response, &questions));
        if let Some(Answer::Choice(answer)) = response.answers.get_mut("q") {
            answer.probabilities.insert("no".into(), 0.0);
            answer.choice = "no".into();
        }
        assert!(!complete_choices(&response, &questions));
    }
}
