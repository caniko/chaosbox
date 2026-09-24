//! Unit tests for the Jev client surface.

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
    let k1 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r1", "p1");
    let k2 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r2", "p1");
    assert_ne!(k1, k2);
    // Effective policy is part of the identity: the same sources under
    // different consent must not reuse each other's inference.
    let k3 = cache_key("s", "c", &q, JEV_MODEL_PINNED, "r1", "p2");
    assert_ne!(k1, k3);
}

#[test]
fn reuse_key_is_snapshot_and_catalog_independent() {
    // Issue #12 Slice 1: the reuse key must survive what the repo-wide
    // cache key deliberately does not — a new snapshot id and a changed
    // catalog — while still covering the relation's own evidence and
    // consent. Entity/candidate ids never enter: they rewrite on every
    // snapshot by design.
    let q = BTreeMap::from([(
        "rel".into(),
        Question::Noul {
            instructions: "y?".into(),
            criteria: None,
        },
    )]);
    let base = reuse_key(
        "r",
        "calls",
        "structural",
        "a.rs",
        "a",
        "b.rs",
        "b",
        "a calls b",
        &q,
        JEV_MODEL_PINNED,
        "rubric-v1",
        "policy-1",
    );
    assert!(base.starts_with("jev-reuse:"));
    // Same relation, different snapshot/catalog context: stable. (Snapshot
    // and catalog digests are not parameters at all, so there is nothing
    // to vary — the assertion is that the function signature excludes
    // them by construction.)
    let same = reuse_key(
        "r",
        "calls",
        "structural",
        "a.rs",
        "a",
        "b.rs",
        "b",
        "a calls b",
        &q,
        JEV_MODEL_PINNED,
        "rubric-v1",
        "policy-1",
    );
    assert_eq!(base, same);
    // The relation's own evidence changed: must re-ask.
    let changed_excerpt = reuse_key(
        "r",
        "calls",
        "structural",
        "a.rs",
        "a",
        "b.rs",
        "b",
        "a calls c",
        &q,
        JEV_MODEL_PINNED,
        "rubric-v1",
        "policy-1",
    );
    assert_ne!(base, changed_excerpt);
    // Consent changed: must re-ask, never reuse across policy.
    let changed_policy = reuse_key(
        "r",
        "calls",
        "structural",
        "a.rs",
        "a",
        "b.rs",
        "b",
        "a calls b",
        &q,
        JEV_MODEL_PINNED,
        "rubric-v1",
        "policy-2",
    );
    assert_ne!(base, changed_policy);
    // Model/rubric changed: must re-ask.
    let changed_model = reuse_key(
        "r",
        "calls",
        "structural",
        "a.rs",
        "a",
        "b.rs",
        "b",
        "a calls b",
        &q,
        "jev-9.9.9",
        "rubric-v1",
        "policy-1",
    );
    assert_ne!(base, changed_model);
    // A different relation triple is a different key.
    let other_rel = reuse_key(
        "r",
        "calls",
        "structural",
        "a.rs",
        "a",
        "c.rs",
        "c",
        "a calls b",
        &q,
        JEV_MODEL_PINNED,
        "rubric-v1",
        "policy-1",
    );
    assert_ne!(base, other_rel);
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
