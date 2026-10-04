//! Unit tests for the Jev client surface.

use super::*;

#[test]
fn production_client_rejects_provider_model_and_endpoint_overrides() {
    for model in ["jev-latest", "jev-9.9.9", "other-provider", "fixture-test"] {
        assert!(JevClient::new(JevPolicy {
            model: model.into(),
            ..JevPolicy::default()
        })
        .is_err());
    }
    for endpoint in [
        "http://api.typesafe.ai/v1/systemone",
        "https://example.org/v1/systemone",
        "http://127.0.0.1:4321/v1/systemone",
        "https://api.typesafe.ai/v1/systemone?proxy=1",
    ] {
        assert!(JevClient::new(JevPolicy {
            endpoint: endpoint.into(),
            ..JevPolicy::default()
        })
        .is_err());
    }
    assert!(JevClient::new(JevPolicy::default()).is_ok());
}

#[test]
fn fixture_identity_cannot_masquerade_as_live_jev() {
    let response = SystemOneResponse {
        model: FIXTURE_MODEL.into(),
        answers: BTreeMap::new(),
        usage: Usage {
            input_tokens: 0,
            output_tokens: 0,
        },
    };
    assert!(validate_response(&response, &BTreeMap::new(), &BTreeMap::new()).is_err());
    assert!(validate_response_for_model(
        &response,
        &BTreeMap::new(),
        &BTreeMap::new(),
        FIXTURE_MODEL
    )
    .is_ok());
    for (requested, returned) in [
        ("", ""),
        ("other", "other"),
        ("jev-latest", JEV_MODEL_PINNED),
        (JEV_MODEL_PINNED, ""),
        (FIXTURE_MODEL, JEV_MODEL_PINNED),
    ] {
        assert!(validate_model_identity(requested, returned).is_err());
    }
}

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
    // Issue #12: the reuse key must survive what the repo-wide cache key
    // deliberately does not — a new snapshot id and a changed catalog —
    // while still covering the relation's own evidence and consent.
    // Snapshot/entity/candidate ids, question map keys, and catalog
    // digests never enter: they rewrite on every snapshot by design.
    let question = || Question::Noul {
        instructions: "y?".into(),
        criteria: None,
    };
    let endpoint = |file: &str, qualified: &str, hash: &str| ReuseEndpoint {
        file: file.into(),
        qualified: qualified.into(),
        kind: "definition".into(),
        file_hash: hash.into(),
    };
    let input_with = |qid: &str, excerpt: &str, policy: &str, model: &str| {
        let ordered = BTreeMap::from([(qid.into(), question())]);
        ReuseInput {
            version: REUSE_VERSION.into(),
            repo: "r".into(),
            rel_type: "calls".into(),
            reason: "structural".into(),
            from: endpoint("a.rs", "a", "hash-a"),
            to: endpoint("b.rs", "b", "hash-b"),
            excerpt: excerpt.into(),
            canonical_questions: canonical_questions(&ordered),
            model: model.into(),
            rubric_version: "rubric-v1".into(),
            policy_digest: policy.into(),
        }
    };
    let base = input_with("rel_cand:1", "a calls b", "policy-1", JEV_MODEL_PINNED);
    let key = reuse_key(&base);
    assert!(key.starts_with("jev-reuse:"));
    // Transport-only question keys differ across snapshots: same semantics
    // must hash identically.
    let renamed_key = input_with("rel_cand:2", "a calls b", "policy-1", JEV_MODEL_PINNED);
    assert_eq!(key, reuse_key(&renamed_key));
    // Canonical values are order-independent: map iteration order never
    // affects the key.
    let multi_a = BTreeMap::from([
        ("rel_1".to_string(), question()),
        (
            "rel_2".to_string(),
            Question::Noul {
                instructions: "z?".into(),
                criteria: None,
            },
        ),
    ]);
    let mut multi_b = multi_a.clone();
    let moved = multi_b.remove("rel_1").unwrap();
    multi_b.insert("rel_transport_differs".to_string(), moved);
    assert_eq!(canonical_questions(&multi_a), canonical_questions(&multi_b));
    // The relation's own excerpt changed: must re-ask.
    assert_ne!(
        key,
        reuse_key(&input_with(
            "rel_cand:1",
            "a calls c",
            "policy-1",
            JEV_MODEL_PINNED
        ))
    );
    // Either endpoint file's bytes changed: must re-ask even when names,
    // excerpt, and questions are unchanged.
    let mut edited_file = base.clone();
    edited_file.to.file_hash = "hash-b-edited".into();
    assert_ne!(key, reuse_key(&edited_file));
    // Consent changed: must re-ask, never reuse across policy.
    assert_ne!(
        key,
        reuse_key(&input_with(
            "rel_cand:1",
            "a calls b",
            "policy-2",
            JEV_MODEL_PINNED
        ))
    );
    // Model/rubric changed: must re-ask.
    assert_ne!(
        key,
        reuse_key(&input_with(
            "rel_cand:1",
            "a calls b",
            "policy-1",
            "jev-9.9.9"
        ))
    );
    let mut edited_rubric = base.clone();
    edited_rubric.rubric_version = "rubric-v2".into();
    assert_ne!(key, reuse_key(&edited_rubric));
    // A different relation triple is a different key.
    let mut other_rel = base.clone();
    other_rel.to = endpoint("c.rs", "c", "hash-c");
    assert_ne!(key, reuse_key(&other_rel));
    // Question semantics changed: must re-ask even under identical keys.
    let mut edited_q = base.clone();
    edited_q.canonical_questions = vec![Question::Noul {
        instructions: "different?".into(),
        criteria: None,
    }];
    assert_ne!(key, reuse_key(&edited_q));
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
