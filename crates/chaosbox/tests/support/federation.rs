//! Validated source-backed fixtures shared by federation contract and live gates.
use chaosbox::intelligence::{Bundle, assess, extract, questions};
use chaosbox_jev::{
    Answer, ChoiceAnswer, JEV_MODEL_PINNED, NoulAnswer, Question, SystemOneResponse, Usage,
};

pub fn bundle(owner: &str, messages: &[(&str, &str, &str)]) -> Bundle {
    related_bundle(owner, messages, None)
}

pub fn related_bundle(
    owner: &str,
    messages: &[(&str, &str, &str)],
    relationship: Option<&str>,
) -> Bundle {
    let mut bundle = Bundle::new(&format!("private:{owner}"));
    for (message, statement, repo) in messages {
        let input = serde_json::json!({"id":message,"type":"user","text":statement});
        let candidate = extract(
            &input.to_string(),
            "opencode",
            "session",
            &bundle.scope,
            &[(*repo).into()],
            20,
        )
        .unwrap()
        .candidates
        .remove(0);
        let (_, asked, _) = questions(&candidate, &bundle).unwrap();
        let novelty = relationship
            .filter(|_| !bundle.records.is_empty())
            .map_or_else(
                || "novel".into(),
                |operation| format!("{operation}:{}", bundle.records[0].id),
            );
        assess(&candidate, &mut bundle, response(asked, &novelty)).unwrap();
    }
    bundle.validate().unwrap();
    bundle
}

fn response(
    asked: std::collections::BTreeMap<String, Question>,
    novelty: &str,
) -> SystemOneResponse {
    let answers = asked
        .into_iter()
        .map(|(name, question)| {
            let answer = match question {
                Question::Noul { .. } => Answer::Noul(NoulAnswer { noul: 0.99 }),
                Question::Choice { criteria, .. } => {
                    let choice = match name.as_str() {
                        "kind" => "constraint",
                        "utility" => "reusable",
                        "novelty" => novelty,
                        _ => panic!("unknown question"),
                    };
                    Answer::Choice(ChoiceAnswer {
                        choice: choice.into(),
                        confidence: 0.99,
                        probabilities: criteria
                            .keys()
                            .map(|key| (key.clone(), f64::from(key == choice)))
                            .collect(),
                    })
                }
                Question::Score { .. } => panic!("unknown question"),
            };
            (name, answer)
        })
        .collect();
    SystemOneResponse {
        model: JEV_MODEL_PINNED.into(),
        answers,
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
        },
    }
}

pub fn withhold(bundle: &mut Bundle, message: &str, statement: &str, repo: &str) {
    let input = serde_json::json!({"id":message,"type":"user","text":statement});
    let candidate = extract(
        &input.to_string(),
        "opencode",
        "session",
        &bundle.scope,
        &[repo.into()],
        20,
    )
    .unwrap()
    .candidates
    .remove(0);
    let (_, asked, _) = questions(&candidate, bundle).unwrap();
    let mut rejected = response(asked, "novel");
    rejected
        .answers
        .insert("support".into(), Answer::Noul(NoulAnswer { noul: 0.01 }));
    assess(&candidate, bundle, rejected).unwrap();
    bundle.validate().unwrap();
}
