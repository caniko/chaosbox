//! Peer reconciliation must converge independently of delivery order.
use chaosbox::sync::{Identity, Publication, Replica, SignedEvent};
use chaosbox::intelligence::{assess, extract, questions, Bundle};
use chaosbox_jev::{
    Answer, ChoiceAnswer, NoulAnswer, Question, SystemOneResponse, Usage, JEV_MODEL_PINNED,
};

fn publication(session: &str, statement: &str) -> Publication {
    let source =
        serde_json::json!({"id":"u1","type":"user","text":statement,"time":{"created":1000}})
            .to_string();
    let candidates = extract(
        &source,
        "opencode",
        session,
        "private:can",
        &["canix".into()],
        20,
    )
    .unwrap();
    let mut bundle = Bundle::new("private:can");
    let (_, asked, _) = questions(&candidates.candidates[0], &bundle).unwrap();
    let response = SystemOneResponse {
        model: JEV_MODEL_PINNED.into(),
        answers: asked
            .into_iter()
            .map(|(id, q)| {
                let a = match q {
                    Question::Noul { .. } => Answer::Noul(NoulAnswer { noul: 0.99 }),
                    Question::Choice { criteria, .. } => {
                        let choice = if id == "kind" { "constraint" } else { "novel" };
                        Answer::Choice(ChoiceAnswer {
                            choice: choice.into(),
                            confidence: 0.99,
                            probabilities: criteria
                                .keys()
                                .map(|k| (k.clone(), f64::from(k == choice)))
                                .collect(),
                        })
                    }
                    Question::Score { .. } => panic!("unexpected score"),
                };
                (id, a)
            })
            .collect(),
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
        },
    };
    assess(&candidates.candidates[0], &mut bundle, response).unwrap();
    Publication {
        bundle,
        candidates: candidates.candidates,
    }
}

#[test]
fn indirect_delivery_and_reordering_converge_without_a_hub() {
    let (a, authority) = Identity::create("private:can").unwrap();
    let b = Identity::enrolled(&authority, "private:can").unwrap();
    let first = SignedEvent::publication(
        &a,
        vec![],
        publication(
            "a",
            "We must preserve original evidence across host synchronization.",
        ),
    )
    .unwrap();
    let second = SignedEvent::publication(
        &b,
        vec![],
        publication(
            "b",
            "We must preserve conflicting findings across host synchronization.",
        ),
    )
    .unwrap();
    let mut ab = Replica::new(&a.user(), "private:can");
    ab.receive(first.clone()).unwrap();
    ab.receive(second.clone()).unwrap();
    let mut ba = Replica::new(&a.user(), "private:can");
    ba.receive(second).unwrap();
    ba.receive(first).unwrap();
    assert_eq!(ab.view().unwrap().digest, ba.view().unwrap().digest);
    let mut relay = Replica::new(&a.user(), "private:can");
    for event in ab.events().values() {
        relay.receive(event.clone()).unwrap();
    }
    assert_eq!(relay.view().unwrap().digest, ab.view().unwrap().digest);
    assert_eq!(relay.view().unwrap().bundle.records.len(), 2);
}

#[test]
fn tampered_and_other_user_events_never_enter_the_replica() {
    let (a, _) = Identity::create("private:can").unwrap();
    let (other, _) = Identity::create("private:can").unwrap();
    let mut event = SignedEvent::publication(
        &a,
        vec![],
        publication(
            "a",
            "We must preserve original evidence across host synchronization.",
        ),
    )
    .unwrap();
    let mut replica = Replica::new(&a.user(), "private:can");
    event.parents.push("not-a-parent".into());
    assert!(replica.receive(event).is_err());
    let foreign = SignedEvent::publication(
        &other,
        vec![],
        publication(
            "b",
            "We must preserve original evidence across host synchronization.",
        ),
    )
    .unwrap();
    assert!(replica.receive(foreign).is_err());
    assert!(replica.events().is_empty());
}

#[test]
fn missing_causal_dependencies_are_staged_not_published() {
    let (a, _) = Identity::create("private:can").unwrap();
    let root = SignedEvent::publication(
        &a,
        vec![],
        publication(
            "a",
            "We must preserve original evidence across host synchronization.",
        ),
    )
    .unwrap();
    let child = SignedEvent::publication(
        &a,
        vec![root.id.clone()],
        publication(
            "b",
            "We must preserve conflicting findings across host synchronization.",
        ),
    )
    .unwrap();
    let mut replica = Replica::new(&a.user(), "private:can");
    replica.receive(child).unwrap();
    assert!(replica.view().is_err());
    replica.receive(root).unwrap();
    assert_eq!(replica.view().unwrap().bundle.records.len(), 2);
    let before = replica.events().len();
    for e in replica.events().clone().into_values() {
        replica.receive(e).unwrap();
    }
    assert_eq!(before, replica.events().len());
}

#[tokio::test]
async fn persistence_keeps_last_good_until_dependencies_arrive_and_isolates_users() {
    use chaosbox::sync::{persist_event, publish_current};
    use chaosbox_store::{MemoryReplicaStore, ReplicaStore};
    let (a, _) = Identity::create("private:can").unwrap();
    let mut store = MemoryReplicaStore::default();
    let root = SignedEvent::publication(
        &a,
        vec![],
        publication(
            "a",
            "We must preserve original evidence across host synchronization.",
        ),
    )
    .unwrap();
    persist_event(&mut store, &a.user(), "private:can", &root)
        .await
        .unwrap();
    let good = publish_current(&mut store, &a.user(), "private:can")
        .await
        .unwrap();
    let parent = SignedEvent::publication(
        &a,
        vec![root.id.clone()],
        publication(
            "b",
            "We must preserve conflicting findings across host synchronization.",
        ),
    )
    .unwrap();
    let child = SignedEvent::publication(
        &a,
        vec![parent.id.clone()],
        publication(
            "c",
            "We must preserve local context during offline synchronization.",
        ),
    )
    .unwrap();
    persist_event(&mut store, &a.user(), "private:can", &child)
        .await
        .unwrap();
    assert!(publish_current(&mut store, &a.user(), "private:can")
        .await
        .is_err());
    assert_eq!(
        store
            .replica_current(&a.user(), "private:can")
            .await
            .unwrap()
            .unwrap()
            .0,
        good.digest
    );
    assert!(store
        .replica_current(&"00".repeat(32), "private:can")
        .await
        .unwrap()
        .is_none());
    persist_event(&mut store, &a.user(), "private:can", &parent)
        .await
        .unwrap();
    assert_eq!(
        publish_current(&mut store, &a.user(), "private:can")
            .await
            .unwrap()
            .bundle
            .records
            .len(),
        3
    );
}

fn relation_answer(job: &chaosbox::sync::Job, choice: &str) -> SystemOneResponse {
    SystemOneResponse {
        model: JEV_MODEL_PINNED.into(),
        usage: Usage {
            input_tokens: 5,
            output_tokens: 2,
        },
        answers: job
            .questions()
            .into_iter()
            .map(|(id, q)| {
                (
                    id,
                    match q {
                        Question::Choice { criteria, .. } => Answer::Choice(ChoiceAnswer {
                            choice: choice.into(),
                            confidence: 0.99,
                            probabilities: criteria
                                .keys()
                                .map(|k| (k.clone(), f64::from(k == choice)))
                                .collect(),
                        }),
                        Question::Noul { .. } => Answer::Noul(NoulAnswer { noul: 0.99 }),
                        Question::Score { .. } => panic!("unexpected score"),
                    },
                )
            })
            .collect(),
    }
}

#[test]
fn conflicting_jev_receipts_converge_to_a_dispute_then_accept_joint_adjudication() {
    use chaosbox::sync::{reconcile_jobs, Resolution, Action};
    let (a, authority) = Identity::create("private:can").unwrap();
    let b = Identity::enrolled(&authority, "private:can").unwrap();
    let mut replica = Replica::new(&a.user(), "private:can");
    replica
        .receive(
            SignedEvent::publication(
                &a,
                vec![],
                publication(
                    "a",
                    "We must preserve original evidence during synchronization.",
                ),
            )
            .unwrap(),
        )
        .unwrap();
    replica
        .receive(
            SignedEvent::publication(
                &b,
                vec![],
                publication(
                    "b",
                    "We must retain original evidence during synchronization.",
                ),
            )
            .unwrap(),
        )
        .unwrap();
    let job = reconcile_jobs(&replica, &replica.view().unwrap(), 10)
        .unwrap()
        .remove(0);
    let first = SignedEvent::resolution(
        &a,
        replica.heads(),
        Resolution::new(job.clone(), relation_answer(&job, "duplicate")).unwrap(),
    )
    .unwrap();
    let second = SignedEvent::resolution(
        &b,
        replica.heads(),
        Resolution::new(job.clone(), relation_answer(&job, "contradiction")).unwrap(),
    )
    .unwrap();
    replica.receive(first).unwrap();
    replica.receive(second).unwrap();
    let view = replica.view().unwrap();
    assert!(view.relationships[0].unresolved);
    assert_eq!(view.relationships[0].action, Action::Abstain);
    let joint = reconcile_jobs(&replica, &view, 10).unwrap().remove(0);
    assert_eq!(joint.prior.len(), 2);
    replica
        .receive(
            SignedEvent::resolution(
                &a,
                replica.heads(),
                Resolution::new(joint.clone(), relation_answer(&joint, "distinct")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let final_view = replica.view().unwrap();
    assert!(!final_view.relationships[0].unresolved);
    assert_eq!(final_view.relationships[0].action, Action::Distinct);
    assert!(reconcile_jobs(&replica, &final_view, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn source_timestamps_on_different_hosts_do_not_authorize_replacement() {
    use chaosbox::sync::{reconcile_jobs, Resolution, Action};
    let (a, _) = Identity::create("private:can").unwrap();
    let mut replica = Replica::new(&a.user(), "private:can");
    for (session, text) in [
        (
            "a",
            "We must keep original evidence during synchronization.",
        ),
        (
            "b",
            "We must discard original evidence during synchronization.",
        ),
    ] {
        replica
            .receive(SignedEvent::publication(&a, vec![], publication(session, text)).unwrap())
            .unwrap();
    }
    let job = reconcile_jobs(&replica, &replica.view().unwrap(), 10)
        .unwrap()
        .remove(0);
    assert_eq!(
        Resolution::new(job.clone(), relation_answer(&job, "left_replaces_right"))
            .unwrap()
            .action,
        Action::Abstain
    );
}

fn reassess(mut p: Publication, support: f64) -> Publication {
    let (_, asked, _) = questions(&p.candidates[0], &p.bundle).unwrap();
    let mut response = p.bundle.assessments[0].response.clone();
    response.answers = asked
        .into_iter()
        .map(|(id, q)| {
            let answer = match q {
                Question::Noul { .. } => Answer::Noul(NoulAnswer {
                    noul: if id == "support" { support } else { 0.99 },
                }),
                Question::Choice { criteria, .. } => {
                    let choice = if id == "kind" { "constraint" } else { "novel" };
                    Answer::Choice(ChoiceAnswer {
                        choice: choice.into(),
                        confidence: 0.99,
                        probabilities: criteria
                            .keys()
                            .map(|k| (k.clone(), f64::from(k == choice)))
                            .collect(),
                    })
                }
                Question::Score { .. } => panic!("unexpected score"),
            };
            (id, answer)
        })
        .collect();
    assess(&p.candidates[0], &mut p.bundle, response).unwrap();
    p
}

#[test]
fn readmission_on_one_branch_does_not_erase_concurrent_withholding() {
    use chaosbox_core::intelligence::IntelligenceStatus;
    let (a, authority) = Identity::create("private:can").unwrap();
    let b = Identity::enrolled(&authority, "private:can").unwrap();
    let original = publication(
        "shared",
        "We must keep original evidence during synchronization.",
    );
    let first_negative = reassess(original.clone(), 0.0);
    let second_negative = reassess(original.clone(), 0.01);
    let readmitted = reassess(first_negative.clone(), 0.98);
    let root = SignedEvent::publication(&a, vec![], original).unwrap();
    let left = SignedEvent::publication(&a, vec![root.id.clone()], first_negative).unwrap();
    let right = SignedEvent::publication(&b, vec![root.id.clone()], second_negative).unwrap();
    let later_left = SignedEvent::publication(&a, vec![left.id.clone()], readmitted).unwrap();
    let mut replica = Replica::new(&a.user(), "private:can");
    for event in [root, left, right, later_left] {
        replica.receive(event).unwrap();
    }
    assert_eq!(
        replica.view().unwrap().bundle.records[0].status,
        IntelligenceStatus::Withheld
    );
}

#[tokio::test]
async fn real_duplex_exchange_pushes_pulls_and_relays_authenticated_events() {
    use chaosbox::sync::{transport, persist_event, publish_current};
    use chaosbox_store::MemoryReplicaStore;
    let (a, authority) = Identity::create("private:can").unwrap();
    let b = Identity::enrolled(&authority, "private:can").unwrap();
    let mut local = MemoryReplicaStore::default();
    let mut remote = MemoryReplicaStore::default();
    for (store, identity, session) in [(&mut local, &a, "a"), (&mut remote, &b, "b")] {
        persist_event(
            store,
            &a.user(),
            "private:can",
            &SignedEvent::publication(
                identity,
                vec![],
                publication(
                    session,
                    "We must preserve original evidence during synchronization.",
                ),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    }
    let (client, server) = tokio::io::duplex(4096);
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut sr, mut sw) = tokio::io::split(server);
    let (pushed, pulled) = tokio::join!(
        transport::exchange(
            &mut local,
            &a,
            "private:can",
            &b.grant.device,
            &mut cr,
            &mut cw
        ),
        transport::serve(&mut remote, &b, "private:can", &mut sr, &mut sw)
    );
    pushed.unwrap();
    pulled.unwrap();
    let left = publish_current(&mut local, &a.user(), "private:can")
        .await
        .unwrap();
    let right = publish_current(&mut remote, &a.user(), "private:can")
        .await
        .unwrap();
    assert_eq!(left.digest, right.digest);
    assert_eq!(left.bundle.records.len(), 2);
}

async fn exchange_pair(
    left: &mut chaosbox_store::MemoryReplicaStore,
    left_id: &Identity,
    right: &mut chaosbox_store::MemoryReplicaStore,
    right_id: &Identity,
) {
    let (a, b) = try_exchange_pair(left, left_id, right, right_id).await;
    a.unwrap();
    b.unwrap();
}

async fn try_exchange_pair(
    left: &mut impl chaosbox_store::ReplicaStore,
    left_id: &Identity,
    right: &mut impl chaosbox_store::ReplicaStore,
    right_id: &Identity,
) -> (Result<(), String>, Result<(), String>) {
    let (client, server) = tokio::io::duplex(4096);
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut sr, mut sw) = tokio::io::split(server);
    tokio::join!(
        async {
            let result = chaosbox::sync::transport::exchange(
                left,
                left_id,
                "private:can",
                &right_id.grant.device,
                &mut cr,
                &mut cw,
            )
            .await;
            drop(cr);
            drop(cw);
            result
        },
        async {
            let result =
                chaosbox::sync::transport::serve(right, right_id, "private:can", &mut sr, &mut sw)
                    .await;
            drop(sr);
            drop(sw);
            result
        }
    )
}

struct InterruptedStore<'a> {
    durable: &'a mut chaosbox_store::MemoryReplicaStore,
    writes: usize,
}

#[async_trait::async_trait]
impl chaosbox_store::ReplicaStore for InterruptedStore<'_> {
    async fn replica_put(
        &mut self,
        row: &chaosbox_store::ReplicaRow,
    ) -> Result<(), chaosbox_store::StoreError> {
        if self.writes == 200 {
            return Err(chaosbox_store::StoreError::Connection(
                "simulated receiver outage after first page".into(),
            ));
        }
        self.writes += 1;
        self.durable.replica_put(row).await
    }
    async fn replica_rows(
        &mut self,
        user: &str,
        scope: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<chaosbox_store::ReplicaRow>, chaosbox_store::StoreError> {
        self.durable.replica_rows(user, scope, after, limit).await
    }
    async fn replica_current(
        &mut self,
        user: &str,
        scope: &str,
    ) -> Result<Option<(String, String)>, chaosbox_store::StoreError> {
        self.durable.replica_current(user, scope).await
    }
    async fn replica_publish(
        &mut self,
        user: &str,
        scope: &str,
        id: &str,
        body: &str,
        expected: Option<&str>,
    ) -> Result<(), chaosbox_store::StoreError> {
        self.durable
            .replica_publish(user, scope, id, body, expected)
            .await
    }
}

#[tokio::test]
async fn offline_three_peer_relay_and_reconnection_converge_over_multiple_pages() {
    use chaosbox::sync::{persist_event, publish_current};
    use chaosbox_store::MemoryReplicaStore;
    let (a, authority) = Identity::create("private:can").unwrap();
    let b = Identity::enrolled(&authority, "private:can").unwrap();
    let c = Identity::enrolled(&authority, "private:can").unwrap();
    let (mut sa, mut sb, mut sc) = (
        MemoryReplicaStore::default(),
        MemoryReplicaStore::default(),
        MemoryReplicaStore::default(),
    );
    for index in 0..201 {
        let event = SignedEvent::publication(
            &a,
            vec![],
            publication(
                &format!("a-{index}"),
                "We must preserve original evidence during synchronization.",
            ),
        )
        .unwrap();
        persist_event(&mut sa, &a.user(), "private:can", &event)
            .await
            .unwrap();
    }
    {
        let mut interrupted = InterruptedStore {
            durable: &mut sb,
            writes: 0,
        };
        let (left, right) = try_exchange_pair(&mut sa, &a, &mut interrupted, &b).await;
        assert!(left.is_err());
        assert!(right.is_err());
    }
    assert_eq!(
        chaosbox::sync::load_replica(&mut sb, &a.user(), "private:can")
            .await
            .unwrap()
            .events()
            .len(),
        200
    );
    exchange_pair(&mut sa, &a, &mut sb, &b).await;
    // A goes offline. B carries A's signed events to C, alongside C's own fork.
    let fork = SignedEvent::publication(
        &c,
        vec![],
        publication(
            "c",
            "We must preserve conflicting evidence during synchronization.",
        ),
    )
    .unwrap();
    persist_event(&mut sc, &a.user(), "private:can", &fork)
        .await
        .unwrap();
    exchange_pair(&mut sb, &b, &mut sc, &c).await;
    exchange_pair(&mut sa, &a, &mut sb, &b).await;
    let va = publish_current(&mut sa, &a.user(), "private:can")
        .await
        .unwrap();
    let vb = publish_current(&mut sb, &a.user(), "private:can")
        .await
        .unwrap();
    let vc = publish_current(&mut sc, &a.user(), "private:can")
        .await
        .unwrap();
    assert_eq!(va.digest, vb.digest);
    assert_eq!(vb.digest, vc.digest);
    assert_eq!(va.bundle.records.len(), 202);
    exchange_pair(&mut sa, &a, &mut sc, &c).await;
    assert_eq!(
        publish_current(&mut sc, &a.user(), "private:can")
            .await
            .unwrap()
            .digest,
        va.digest
    );
}

#[test]
fn enrollment_cli_keeps_root_authority_local_and_installs_only_matching_grants() {
    fn cli(directory: &std::path::Path, args: &[&str]) -> serde_json::Value {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
            .args(["sync", "--directory"])
            .arg(directory)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let first = cli(&a, &["init", "--scope", "private:can"]);
    let next = cli(
        &b,
        &[
            "init",
            "--scope",
            "private:can",
            "--user",
            first["user"].as_str().unwrap(),
        ],
    );
    assert!(!b.join("authority.pk8").exists());
    let certificate = a.join("certificate.json");
    cli(
        &a,
        &[
            "authorize",
            next["device"].as_str().unwrap(),
            "--output",
            certificate.to_str().unwrap(),
        ],
    );
    assert_eq!(
        cli(&b, &["enroll", certificate.to_str().unwrap()])["enrolled"],
        true
    );
    cli(
        &b,
        &[
            "peer",
            "--target",
            "can@peer",
            "--device",
            first["device"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        chaosbox::sync::cli::Settings::load(&b).unwrap().peers.len(),
        1
    );
    let rejected = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(["sync", "--directory"])
        .arg(&a)
        .args(["enroll", certificate.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !rejected.status.success(),
        "another device's grant must not replace this identity"
    );
}

struct FailingInference(usize);
#[async_trait::async_trait]
impl chaosbox::Responder for FailingInference {
    async fn respond(
        &mut self,
        _: serde_json::Value,
        _: std::collections::BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, String> {
        self.0 += 1;
        Err("simulated interrupted dispatch".into())
    }
}

#[tokio::test]
async fn unknown_inference_outcome_keeps_its_charge_across_worker_restarts() {
    use chaosbox::sync::{persist_event, reconcile, ReconcileBudget};
    let (a, _) = Identity::create("private:can").unwrap();
    let mut store = chaosbox_store::MemoryReplicaStore::default();
    for session in ["a", "b"] {
        persist_event(
            &mut store,
            &a.user(),
            "private:can",
            &SignedEvent::publication(
                &a,
                vec![],
                publication(
                    session,
                    "We must preserve original evidence during synchronization.",
                ),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    }
    let mut responder = FailingInference(0);
    let mut budget = ReconcileBudget {
        epoch: "test".into(),
        requests: 1,
        input_tokens: 1_000_000,
        max_jobs: 20,
        retry: false,
    };
    assert!(
        reconcile(&mut store, &a, "private:can", &mut responder, &budget)
            .await
            .is_err()
    );
    assert_eq!(responder.0, 1);
    let mut restarted = FailingInference(0);
    assert_eq!(
        reconcile(&mut store, &a, "private:can", &mut restarted, &budget)
            .await
            .unwrap(),
        0
    );
    assert_eq!(restarted.0, 0);
    budget.retry = true;
    assert!(
        reconcile(&mut store, &a, "private:can", &mut restarted, &budget)
            .await
            .is_err()
    );
    assert_eq!(
        restarted.0, 0,
        "explicit retries still obey the durable spending ceiling"
    );
    budget.retry = false;
    budget.requests = 2;
    budget.max_jobs = 1;
    let new = SignedEvent::publication(
        &a,
        vec![],
        publication(
            "c",
            "We must preserve original evidence during synchronization.",
        ),
    )
    .unwrap();
    persist_event(&mut store, &a.user(), "private:can", &new)
        .await
        .unwrap();
    assert!(
        reconcile(&mut store, &a, "private:can", &mut restarted, &budget)
            .await
            .is_err()
    );
    assert_eq!(
        restarted.0, 1,
        "deferred attempts must not starve new jobs behind the page limit"
    );
}

#[tokio::test]
async fn exchange_refuses_an_unexpected_enrolled_device_before_accepting_events() {
    use chaosbox::sync::transport;
    use chaosbox_store::MemoryReplicaStore;
    let (a, authority) = Identity::create("private:can").unwrap();
    let b = Identity::enrolled(&authority, "private:can").unwrap();
    let mut left = MemoryReplicaStore::default();
    let mut right = MemoryReplicaStore::default();
    let (client, server) = tokio::io::duplex(4096);
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut sr, mut sw) = tokio::io::split(server);
    let (client_result, server_result) = tokio::join!(
        async {
            let result = transport::exchange(
                &mut left,
                &a,
                "private:can",
                &a.grant.device,
                &mut cr,
                &mut cw,
            )
            .await;
            drop(cr);
            drop(cw);
            result
        },
        async {
            let result = transport::serve(&mut right, &b, "private:can", &mut sr, &mut sw).await;
            drop(sr);
            drop(sw);
            result
        }
    );
    assert!(client_result.is_err());
    assert!(server_result.is_err());
    assert!(
        chaosbox::sync::load_replica(&mut left, &a.user(), "private:can")
            .await
            .unwrap()
            .events()
            .is_empty()
    );
}

#[test]
fn publisher_verifies_capsules_against_the_original_normalized_source() {
    use chaosbox::sync::publication_from_sources;
    let text = "We must preserve original evidence during synchronization.";
    let original = serde_json::json!({"id":"u1","type":"user","text":text,"time":{"created":1000}})
        .to_string();
    let p = publication("a", text);
    let verified = publication_from_sources(p.bundle.clone(), |_| Ok(original.clone())).unwrap();
    assert_eq!(verified.candidates[0], p.candidates[0]);
    assert!(publication_from_sources(p.bundle, |_| Ok("altered source".into())).is_err());
}

#[test]
fn published_metadata_must_replay_from_the_retained_receipts() {
    use chaosbox_core::intelligence::IntelligenceKind;
    let mut p = publication(
        "a",
        "We must preserve original evidence during synchronization.",
    );
    p.bundle.records[0].kind = IntelligenceKind::Pitfall;
    assert!(
        p.validate("private:can").is_err(),
        "kind cannot be forged independently of Jev's admission"
    );
    let mut p = publication(
        "a",
        "We must preserve original evidence during synchronization.",
    );
    let other = publication(
        "b",
        "We must preserve conflicting evidence during synchronization.",
    );
    p.bundle.records.extend(other.bundle.records);
    p.bundle.assessments.extend(other.bundle.assessments);
    p.candidates.extend(other.candidates);
    let first = p.bundle.records[0].id.clone();
    let second = p.bundle.records[1].id.clone();
    p.bundle.records[0].contradicts.push(second);
    p.bundle.records[1].contradicts.push(first);
    for record in &mut p.bundle.records {
        record.status = chaosbox_core::intelligence::IntelligenceStatus::Disputed;
    }
    assert!(
        p.validate("private:can").is_err(),
        "relationships cannot be invented outside model receipts"
    );
}

#[tokio::test]
#[ignore = "requires disposable TypeDB; scripts/test-typedb.sh runs this gate"]
async fn signed_history_and_live_consumers_survive_real_typedb_restarts() {
    use chaosbox::sync::{persist_event, publish_current, current::CurrentReader};
    use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};
    let mut config = TypeDbConfig::from_env().unwrap();
    config.database = format!("sync_live_{}", std::process::id());
    let mut store = TypeDbStore::new(config.clone());
    store.migrate().await.unwrap();
    let (identity, _) = Identity::create("private:can").unwrap();
    let first = SignedEvent::publication(
        &identity,
        vec![],
        publication(
            "a",
            "We must preserve original evidence during synchronization.",
        ),
    )
    .unwrap();
    persist_event(&mut store, &identity.user(), "private:can", &first)
        .await
        .unwrap();
    let initial = publish_current(&mut store, &identity.user(), "private:can")
        .await
        .unwrap();
    let mut reader = CurrentReader::new(
        Box::new(TypeDbStore::new(config.clone())),
        &identity.user(),
        "private:can",
    );
    let args = serde_json::json!({"repo":"canix","query":"evidence synchronization"});
    assert_eq!(
        reader.query("intelligence_context", &args).await.unwrap()["snapshot"],
        initial.digest
    );
    let second = SignedEvent::publication(
        &identity,
        vec![first.id],
        publication(
            "b",
            "We must preserve conflicting evidence during synchronization.",
        ),
    )
    .unwrap();
    persist_event(&mut store, &identity.user(), "private:can", &second)
        .await
        .unwrap();
    let next = publish_current(&mut store, &identity.user(), "private:can")
        .await
        .unwrap();
    let fresh = reader.query("intelligence_context", &args).await.unwrap();
    assert_eq!(fresh["snapshot"], next.digest);
    assert_eq!(fresh["records"].as_array().unwrap().len(), 2);
    drop(store);
    let mut reopened = TypeDbStore::new(config);
    assert_eq!(
        publish_current(&mut reopened, &identity.user(), "private:can")
            .await
            .unwrap()
            .digest,
        next.digest
    );
    assert!(reader
        .query(
            "intelligence_context",
            &serde_json::json!({"repo":"canix","query":"evidence","scope":"private:other"})
        )
        .await
        .is_err());
}
