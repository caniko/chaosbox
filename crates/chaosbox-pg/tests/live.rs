//! Live PostgreSQL backend tests: write/publish/readback, decision
//! supersedure, predecessor guards, publication races, knowledge and replica
//! pointers, and pgvector nearest-neighbour reads. Each test owns a disposable
//! cluster (see `support/server.rs`); nothing skips once selected.
//!
//! Run: `cargo test -p chaosbox-pg -- --ignored` with `initdb`/`pg_ctl`/`psql`
//! from a PostgreSQL build that ships pgvector (or `PG_BIN=<its bin dir>`).

#[path = "support/server.rs"]
mod server;

use std::collections::BTreeMap;

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, EntityKind, Evidence, EvidenceClass,
    GraphBuild, InferenceRecord, RawAnswer, Relation, RelationScope, RelationType, SnapshotFile,
    SourceSpan,
};
use chaosbox_pg::{reader::PgReader, store::PgStore, PgConfig};
use chaosbox_store::{check_conformance, check_write_conformance, GraphQueries, Store};
use server::Server;

fn config(server: &Server) -> PgConfig {
    PgConfig::new(server.url()).unwrap()
}

async fn open(server: &Server) -> PgStore {
    let mut s = PgStore::new(config(server));
    s.migrate().await.unwrap();
    s
}

async fn reader(server: &Server) -> PgReader {
    let mut r = PgReader::new(config(server));
    r.connect().await.unwrap();
    r
}

fn span(file: &str) -> SourceSpan {
    SourceSpan::point(file, 1, 1, 0)
}

fn ent(repo: &str, snap: &str, file: &str, name: &str) -> Entity {
    Entity::new(EntityKind::Symbol, repo, snap, file, name, name, span(file))
}

fn decision(candidate_id: &str, question: &str, cache_key: &str) -> Decision {
    Decision {
        id: format!("dec:{candidate_id}:{question}"),
        candidate_id: candidate_id.into(),
        question_id: question.into(),
        outcome: DecisionOutcome::Accepted,
        evidence_class: EvidenceClass::Extracted,
        model_requested: "jev-1.13.0".into(),
        model_returned: "jev-1.13.0".into(),
        confidence: Some(0.9),
        probability: Some(0.8),
        cache_key: cache_key.into(),
        reuse_key: String::new(),
        raw_answer: None,
    }
}

fn choice(choice: &str, confidence: f64) -> RawAnswer {
    RawAnswer::Choice {
        choice: choice.into(),
        probabilities: BTreeMap::from([
            ("accept".into(), if choice == "accept" { 0.9 } else { 0.05 }),
            ("reject".into(), if choice == "reject" { 0.9 } else { 0.05 }),
            ("none".into(), 0.05),
        ]),
        confidence,
    }
}

fn inference(reuse_key: &str, raw: RawAnswer) -> InferenceRecord {
    InferenceRecord {
        reuse_key: reuse_key.into(),
        raw,
        model_requested: "jev-1.13.0".into(),
        model_returned: "jev-1.13.0".into(),
    }
}

fn evidence(id: &str, snapshot: &str, supports: bool) -> Evidence {
    Evidence {
        id: id.into(),
        class: EvidenceClass::Extracted,
        supports,
        text: id.into(),
        span: None,
        snapshot: snapshot.into(),
        source_file_version: "a.rs".into(),
        producer: None,
    }
}

async fn seed_files_run(s: &mut PgStore, repo: &str, snap: &str) -> String {
    let files = vec![SnapshotFile {
        snapshot: snap.into(),
        path: "a.rs".into(),
        sha256: "abc".into(),
        bytes: 10,
    }];
    s.ensure_snapshot_files(snap, repo, &files).await.unwrap();
    let set = format!("set-{snap}");
    s.ensure_run(
        &format!("run-{snap}"),
        repo,
        snap,
        &set,
        "digest-1",
        "rubric-v1",
    )
    .await
    .unwrap();
    set
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_migrate_is_idempotent_and_serialized() {
    let server = Server::start();
    let (mut a, mut b) = (PgStore::new(config(&server)), PgStore::new(config(&server)));
    let (ra, rb) = tokio::join!(a.migrate(), b.migrate());
    ra.unwrap();
    rb.unwrap();
    a.migrate().await.unwrap();
    assert!(reader(&server).await.probe().await.unwrap());
    let empty = Server::start();
    assert!(!reader(&empty).await.probe().await.unwrap(), "unmigrated");
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_store_passes_write_conformance() {
    let server = Server::start();
    check_write_conformance(&mut open(&server).await).await;
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_reader_passes_reference_conformance() {
    let server = Server::start();
    let mut s = open(&server).await;
    for snap in ["s1", "s2"] {
        s.ensure_snapshot_files(
            snap,
            "conf",
            &[SnapshotFile {
                snapshot: snap.into(),
                path: "f.rs".into(),
                sha256: "ff".into(),
                bytes: 8,
            }],
        )
        .await
        .unwrap();
    }
    let ent = |snap: &str, file: &str, name: &str| {
        Entity::new(
            EntityKind::Symbol,
            "conf",
            snap,
            file,
            name,
            name,
            span(file),
        )
    };
    let mut b1 = GraphBuild::new("conf", vec!["s1".into()], 1);
    let a1 = ent("s1", "f.rs", "Alpha");
    let b1e = ent("s1", "f.rs", "Beta");
    b1.add_node(a1.clone()).unwrap();
    b1.add_node(b1e.clone()).unwrap();
    let r1 = Relation::new(
        RelationType::Calls,
        &a1.id,
        &b1e.id,
        RelationScope::File,
        &b1.id,
    );
    b1.add_edge(r1.clone()).unwrap();
    let mut b2 = GraphBuild::new("conf", vec!["s2".into()], 2);
    b2.predecessor = Some(b1.id.clone());
    let a2 = ent("s2", "f.rs", "Alpha");
    let c2 = ent("s2", "g.rs", "Gamma");
    b2.add_node(a2.clone()).unwrap();
    b2.add_node(c2.clone()).unwrap();
    b2.add_edge(Relation::new(
        RelationType::References,
        &a2.id,
        &c2.id,
        RelationScope::CrossFile,
        &b2.id,
    ))
    .unwrap();
    s.put_evidence(Evidence {
        id: "ev1".into(),
        class: EvidenceClass::Extracted,
        supports: true,
        text: "[structural] Alpha -> Beta".into(),
        span: None,
        snapshot: "s1".into(),
        source_file_version: "f.rs".into(),
        producer: None,
    })
    .await
    .unwrap();
    s.put_claim(Claim {
        id: "claim:conf1".into(),
        relation_id: r1.id.clone(),
        supporting: vec!["ev1".into()],
        contradicting: vec![],
        accepted: true,
    })
    .await
    .unwrap();
    s.publish(b1.clone(), None).await.unwrap();
    s.publish(b2.clone(), Some(b1.id.clone())).await.unwrap();
    // A second repository never leaks into another repository's pin.
    s.ensure_snapshot_files(
        "s9",
        "other",
        &[SnapshotFile {
            snapshot: "s9".into(),
            path: "z.rs".into(),
            sha256: "zz".into(),
            bytes: 3,
        }],
    )
    .await
    .unwrap();
    let mut ob = GraphBuild::new("other", vec!["s9".into()], 1);
    ob.add_node(Entity::new(
        EntityKind::Symbol,
        "other",
        "s9",
        "z.rs",
        "Zeta",
        "Zeta",
        span("z.rs"),
    ))
    .unwrap();
    s.publish(ob, None).await.unwrap();
    let reader = reader(&server).await;
    assert_eq!(
        reader
            .active_build("conf")
            .await
            .unwrap()
            .unwrap()
            .snapshots,
        ["s2"]
    );
    assert_eq!(
        reader
            .active_build("other")
            .await
            .unwrap()
            .unwrap()
            .snapshots,
        ["s9"]
    );
    // A pointer naming another repository's build is not an active build.
    server.psql(
        "chaosbox",
        &format!(
            "UPDATE active_pointer SET build_id = '{}' WHERE repo = 'other'",
            b2.id
        ),
    );
    assert!(reader.active_build("other").await.unwrap().is_none());
    check_conformance(
        &reader,
        &a1.id,
        &b1e.id,
        &r1.id,
        &a2.id,
        &(b1.id.clone(), b2.id.clone()),
    )
    .await;
    // Neighbor counts and pages are not covered by the shared suite.
    assert_eq!(
        reader
            .neighbor_counts(&b1.id, &a1.id, vec!["calls".into()])
            .await
            .unwrap(),
        (1, 0)
    );
    let page = reader
        .neighbor_page(
            &b1.id,
            &b1e.id,
            chaosbox_store::NeighborhoodDirection::Incoming,
            vec!["calls".into()],
            None,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
    assert!(reader
        .neighbor_page(
            &b1.id,
            &b1e.id,
            chaosbox_store::NeighborhoodDirection::Incoming,
            vec!["calls".into()],
            Some(&page[0].rel_id),
            10,
        )
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
#[allow(clippy::too_many_lines)]
async fn live_pg_direct_fact_citations_survive_a_fresh_reader_and_later_claims() {
    use chaosbox_core::coverage::{BuildCoverage, FileCoverage, SyntaxStatus};
    let server = Server::start();
    let mut store = open(&server).await;
    let repo = "facts";
    let file = ent(repo, "s1", "a.rs", "file");
    let definition = ent(repo, "s1", "a.rs", "declaration");
    store
        .ensure_snapshot_files(
            "s1",
            repo,
            &[SnapshotFile {
                snapshot: "s1".into(),
                path: "a.rs".into(),
                sha256: "known-content-hash".into(),
                bytes: 40,
            }],
        )
        .await
        .unwrap();
    let mut build = GraphBuild::new(repo, vec!["s1".into()], 1);
    build.add_node(file.clone()).unwrap();
    build.add_node(definition.clone()).unwrap();
    let mut rel = Relation::new(
        RelationType::Defines,
        &file.id,
        &definition.id,
        RelationScope::File,
        &build.id,
    );
    let source_span = SourceSpan {
        file: "a.rs".into(),
        start_line: 2,
        start_col: 4,
        end_line: 2,
        end_col: 15,
        byte_start: 10,
        byte_end: 21,
    };
    let direct = Evidence {
        id: "ev:direct".into(),
        span: Some(source_span.clone()),
        producer: Some("declarations-test-v1".into()),
        ..evidence("ev:direct", "s1", true)
    };
    store.put_evidence(direct.clone()).await.unwrap();
    rel.evidence_ids.push(direct.id.clone());
    store
        .put_claim(Claim {
            id: "claim:direct".into(),
            relation_id: rel.id.clone(),
            supporting: vec![direct.id],
            contradicting: vec![],
            accepted: true,
        })
        .await
        .unwrap();
    build.add_edge(rel.clone()).unwrap();
    build.coverage = Some(BuildCoverage {
        catalog: None,
        compiler: None,
        files: vec![FileCoverage {
            file: "a.rs".into(),
            producer: "declarations-test-v1".into(),
            status: SyntaxStatus::Parsed,
            facts: 1,
        }],
        structural_relations: 1,
        decision_relations: 0,
    });
    store.publish(build.clone(), None).await.unwrap();
    let r = reader(&server).await;
    assert_eq!(
        r.active_build(repo).await.unwrap().unwrap().coverage,
        build.coverage
    );
    let rows = r.evidence_for(&build.id, &rel.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].producer.as_deref(), Some("declarations-test-v1"));
    let citation = rows[0].citation.as_ref().unwrap();
    assert_eq!(citation.sha256, "known-content-hash");
    assert_eq!(citation.span.as_ref(), Some(&source_span));
    assert_eq!(
        r.entity_by_id(&build.id, &definition.id)
            .await
            .unwrap()
            .unwrap()
            .span,
        Some(definition.span.clone())
    );
    assert!(r
        .evidence_for("build:other", &rel.id)
        .await
        .unwrap()
        .is_empty());
    // A later claim and publication never change the sealed history.
    store
        .put_claim(Claim {
            id: "claim:missing".into(),
            relation_id: rel.id.clone(),
            supporting: vec!["ev:direct".into()],
            contradicting: vec!["ev:absent".into()],
            accepted: true,
        })
        .await
        .expect_err("a missing contradiction must not disappear");
    store
        .put_evidence(evidence("ev:later", "s1", false))
        .await
        .unwrap();
    store
        .put_claim(Claim {
            id: "claim:later".into(),
            relation_id: rel.id.clone(),
            supporting: vec![],
            contradicting: vec!["ev:later".into()],
            accepted: false,
        })
        .await
        .unwrap();
    let next = GraphBuild::new(repo, vec!["s1".into()], 2);
    store.publish(next, Some(build.id.clone())).await.unwrap();
    let rows = r.evidence_for(&build.id, &rel.id).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "historical evidence changed after publication"
    );
    assert_eq!(rows[0].evidence_id, "ev:direct");
    assert!(r
        .published_build("foreign", &build.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_publish_readback_and_predecessor_guards() {
    let server = Server::start();
    let mut s = open(&server).await;
    let repo = "pubrepo";
    let set = seed_files_run(&mut s, repo, "s1").await;
    let a = ent(repo, "s1", "a.rs", "a");
    let b = ent(repo, "s1", "a.rs", "b");
    s.put_candidate(
        &set,
        &Candidate {
            id: "cand:1".into(),
            rel_type: RelationType::Calls,
            from_entity: a.id.clone(),
            to_entity: b.id.clone(),
            reason: "structural".into(),
            state_excerpt: "fn a() {}".into(),
        },
    )
    .await
    .unwrap();
    s.put_decision(decision("cand:1", "q1", "key-1"))
        .await
        .unwrap();
    s.put_evidence(evidence("ev:1", "s1", true)).await.unwrap();
    let mut build = GraphBuild::new(repo, vec!["s1".into()], 1);
    build.add_node(a.clone()).unwrap();
    build.add_node(b.clone()).unwrap();
    let bid = build.id.clone();
    build
        .add_edge(Relation::new(
            RelationType::Calls,
            &a.id,
            &b.id,
            RelationScope::CrossFile,
            &bid,
        ))
        .unwrap();
    s.publish(build.clone(), None).await.unwrap();
    let fresh = open(&server).await;
    let found = fresh.find_decision("cand:1", "q1").await.unwrap().unwrap();
    assert_eq!(
        (found.cache_key.as_str(), found.id.as_str()),
        ("key-1", "dec:cand:1:q1")
    );
    assert_eq!(
        server.psql("chaosbox", "SELECT count(*) FROM evidence"),
        "1\n"
    );
    assert_eq!(
        server.psql("chaosbox", "SELECT count(*) FROM candidates"),
        "1\n"
    );
    let mut build2 = GraphBuild::new(repo, vec!["s1".into()], 2);
    build2.predecessor = Some(bid.clone());
    build2.add_node(a.clone()).unwrap();
    build2.add_node(b.clone()).unwrap();
    s.publish(build2.clone(), Some(bid.clone())).await.unwrap();
    let mut stale = GraphBuild::new(repo, vec!["s1".into()], 3);
    stale.add_node(a.clone()).unwrap();
    let err = s
        .publish(stale, Some("build:stale".into()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("predecessor mismatch"), "{err}");
    let mut older = GraphBuild::new(repo, vec!["s1".into()], 1);
    older.predecessor = Some(build2.id.clone());
    older.add_node(a.clone()).unwrap();
    let err = s.publish(older, Some(build2.id.clone())).await.unwrap_err();
    assert!(err.to_string().contains("older worker"), "{err}");
    assert_eq!(
        reader(&server)
            .await
            .active_build(repo)
            .await
            .unwrap()
            .unwrap()
            .build_id,
        build2.id
    );
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_concurrent_publishers_from_same_generation_exactly_one_wins() {
    let server = Server::start();
    let mut s1 = open(&server).await;
    let repo = "racerepo";
    seed_files_run(&mut s1, repo, "s1").await;
    let a = ent(repo, "s1", "a.rs", "a");
    let mut gen1 = GraphBuild::new(repo, vec!["s1".into()], 1);
    gen1.add_node(a.clone()).unwrap();
    s1.publish(gen1.clone(), None).await.unwrap();
    // Exact retries are idempotent, from this or a fresh process.
    s1.publish(gen1.clone(), Some(gen1.id.clone()))
        .await
        .unwrap();
    open(&server)
        .await
        .publish(gen1.clone(), None)
        .await
        .unwrap();
    let mut b1 = GraphBuild::new(repo, vec!["s1".into()], 2);
    b1.predecessor = Some(gen1.id.clone());
    b1.add_node(a.clone()).unwrap();
    let mut b2 = GraphBuild::new(repo, vec!["s2".into()], 2);
    b2.predecessor = Some(gen1.id.clone());
    b2.add_node(a.clone()).unwrap();
    assert_ne!(b1.id, b2.id);
    let (mut w1, mut w2) = (open(&server).await, open(&server).await);
    let (r1, r2) = tokio::join!(
        w1.publish(b1.clone(), Some(gen1.id.clone())),
        w2.publish(b2.clone(), Some(gen1.id.clone()))
    );
    assert!(
        r1.is_ok() ^ r2.is_ok(),
        "exactly one publisher wins: {r1:?} vs {r2:?}"
    );
    let stale = if r1.is_ok() { b2 } else { b1 };
    let err = open(&server)
        .await
        .publish(stale, Some(gen1.id.clone()))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("predecessor mismatch") || err.contains("concurrent publisher"),
        "{err}"
    );
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_same_identity_rival_graphs_cannot_mutate_published_membership() {
    let server = Server::start();
    let (mut left, mut right) = (open(&server).await, open(&server).await);
    let mut a = GraphBuild::new("rivals", vec!["snapshot".into()], 1);
    a.add_node(ent("rivals", "snapshot", "a.rs", "a")).unwrap();
    let mut b = a.clone();
    b.add_node(ent("rivals", "snapshot", "b.rs", "b")).unwrap();
    assert_eq!(a.id, b.id);
    let (l, r) = tokio::join!(
        left.publish(a.clone(), None),
        right.publish(b.clone(), None)
    );
    assert!(
        l.is_ok() ^ r.is_ok(),
        "different versions shared an identity: {l:?}, {r:?}"
    );
    let (winner, loser) = if l.is_ok() { (a, b) } else { (b, a) };
    let reader = reader(&server).await;
    let before =
        serde_json::to_value(reader.build_entities(&winner.id, 10).await.unwrap()).unwrap();
    assert!(open(&server).await.publish(loser, None).await.is_err());
    let after = serde_json::to_value(reader.build_entities(&winner.id, 10).await.unwrap()).unwrap();
    assert_eq!(before, after);
    assert_eq!(after.as_array().unwrap().len(), winner.nodes.len());
    open(&server).await.publish(winner, None).await.unwrap();
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_claim_payload_is_immutable_across_store_restarts() {
    let server = Server::start();
    let mut first = open(&server).await;
    seed_files_run(&mut first, "claims", "s1").await;
    first
        .put_evidence(evidence("support", "s1", true))
        .await
        .unwrap();
    let original = Claim {
        id: "stable-claim".into(),
        relation_id: "rel".into(),
        supporting: vec!["support".into()],
        contradicting: vec![],
        accepted: true,
    };
    first.put_claim(original.clone()).await.unwrap();
    first
        .publish(GraphBuild::new("claims", vec!["s1".into()], 1), None)
        .await
        .unwrap();
    let mut restarted = open(&server).await;
    seed_files_run(&mut restarted, "claims", "s1").await;
    restarted
        .put_evidence(evidence("new-ref", "s1", false))
        .await
        .unwrap();
    for conflict in [
        Claim {
            relation_id: "different-rel".into(),
            ..original.clone()
        },
        Claim {
            accepted: false,
            ..original.clone()
        },
        Claim {
            supporting: vec!["new-ref".into()],
            ..original.clone()
        },
        Claim {
            contradicting: vec!["new-ref".into()],
            ..original.clone()
        },
    ] {
        assert!(restarted.put_claim(conflict).await.is_err());
    }
    assert_eq!(
        server.psql("chaosbox", "SELECT body FROM claims"),
        format!("{}\n", serde_json::to_string(&original).unwrap())
    );
}

async fn stage_racing_claim(store: &mut PgStore, snapshot: &str, supports: bool) -> Claim {
    seed_files_run(store, snapshot, snapshot).await;
    store
        .put_evidence(evidence(snapshot, snapshot, supports))
        .await
        .unwrap();
    let refs = vec![snapshot.to_owned()];
    let claim = Claim {
        id: "racing-claim".into(),
        relation_id: "rel".into(),
        supporting: if supports { refs.clone() } else { vec![] },
        contradicting: if supports { vec![] } else { refs },
        accepted: supports,
    };
    store.put_claim(claim.clone()).await.unwrap();
    claim
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_conflicting_claims_staged_before_publication_cannot_both_publish() {
    let server = Server::start();
    let (mut left, mut right) = (open(&server).await, open(&server).await);
    let a = stage_racing_claim(&mut left, "left", true).await;
    let b = stage_racing_claim(&mut right, "right", false).await;
    let (l, r) = tokio::join!(
        left.publish(GraphBuild::new("left", vec!["left".into()], 1), None),
        right.publish(GraphBuild::new("right", vec!["right".into()], 1), None)
    );
    assert!(
        l.is_ok() ^ r.is_ok(),
        "conflicting claim payloads both published: {l:?}, {r:?}"
    );
    let winner = if l.is_ok() { a } else { b };
    assert_eq!(
        server.psql(
            "chaosbox",
            "SELECT body FROM claims WHERE claim_id = 'racing-claim'"
        ),
        format!("{}\n", serde_json::to_string(&winner).unwrap())
    );
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_decisions_and_inferences_are_durable_first_and_converge() {
    let server = Server::start();
    let mut s = open(&server).await;
    // First write wins for inferences, across racing stores and restarts.
    let first = inference("jev-reuse:1", choice("accept", 0.95));
    s.put_inference(first.clone()).await.unwrap();
    s.put_inference(inference("jev-reuse:1", choice("reject", 0.9)))
        .await
        .unwrap();
    let (mut a, mut b) = (open(&server).await, open(&server).await);
    let (ra, rb) = tokio::join!(
        a.put_inference(inference("jev-reuse:race", choice("accept", 0.95))),
        b.put_inference(inference("jev-reuse:race", choice("reject", 0.9)))
    );
    ra.unwrap();
    rb.unwrap();
    let winner = a.find_inference("jev-reuse:race").await.unwrap().unwrap();
    assert_eq!(
        winner.raw,
        b.find_inference("jev-reuse:race")
            .await
            .unwrap()
            .unwrap()
            .raw
    );
    // Threshold/policy changes replace a decision; the same key keeps it.
    let mut old = decision("cand:1", "q1", "key-old");
    old.outcome = DecisionOutcome::Abstained;
    s.put_decision(old).await.unwrap();
    let mut new = decision("cand:1", "q1", "key-new");
    new.id = "dec:new".into();
    new.reuse_key = "jev-reuse:1".into();
    new.raw_answer = Some(first.raw.clone());
    s.put_decision(new.clone()).await.unwrap();
    let mut same_key = decision("cand:1", "q1", "key-new");
    same_key.outcome = DecisionOutcome::Rejected;
    s.put_decision(same_key).await.unwrap();
    // A recorded failure is retried even under the same cache key.
    s.put_decision(Decision {
        outcome: DecisionOutcome::Failed("down".into()),
        ..decision("cand:2", "q1", "k")
    })
    .await
    .unwrap();
    s.put_decision(decision("cand:2", "q1", "k")).await.unwrap();
    let fresh = open(&server).await;
    assert_eq!(
        fresh.find_decision("cand:1", "q1").await.unwrap().unwrap(),
        new
    );
    assert_eq!(
        fresh
            .find_decision("cand:2", "q1")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        DecisionOutcome::Accepted
    );
    assert_eq!(
        fresh.find_inference("jev-reuse:1").await.unwrap().unwrap(),
        first
    );
    // Non-finite values fail before anything durable or staged exists.
    let bad = inference("jev-reuse:bad", choice("accept", f64::NAN));
    assert!(s.put_inference(bad).await.is_err());
    assert!(s.find_inference("jev-reuse:bad").await.unwrap().is_none());
    let mut d = decision("cand:bad", "q1", "key-bad");
    d.confidence = Some(f64::INFINITY);
    assert!(s.put_decision(d).await.is_err());
    assert!(s.find_decision("cand:bad", "q1").await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
#[allow(clippy::too_many_lines)]
async fn live_pg_private_session_knowledge_is_pinned_idempotent_and_predecessor_guarded() {
    use chaosbox_core::intelligence::{
        Intelligence, IntelligenceKind, IntelligenceStatus, SessionEvidence,
    };
    let server = Server::start();
    let mut store = open(&server).await;
    let scope = "private:test-owner";
    let item = Intelligence {
        id: "intel:fixture".into(),
        scope: scope.into(),
        repositories: vec!["test-repo".into()],
        statement: "Preserve explicit operator-selected private scope.".into(),
        kind: IntelligenceKind::Constraint,
        status: IntelligenceStatus::Admitted,
        interpretation_class: EvidenceClass::Inferred,
        evidence: vec![SessionEvidence {
            source: "opencode".into(),
            snapshot: "a".repeat(64),
            session: "ses_native".into(),
            message: "msg_native".into(),
            pointer: "/text".into(),
            line: 1,
            quote: "Preserve explicit operator-selected private scope.".into(),
            speaker: "user".into(),
            observed_at_ms: Some(1),
        }],
        contradicts: vec![],
        supersedes: None,
        assessments: vec!["fixture-receipt".into()],
    };
    let first =
        serde_json::json!({"scope":scope,"records":[item],"assessments":["fixture-receipt"]})
            .to_string();
    let records = vec![item];
    let id = store
        .publish_knowledge(scope, &first, &records, None)
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_knowledge(scope, &first, &records, None)
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        store.knowledge(scope).await.unwrap(),
        Some((id.clone(), first.clone()))
    );
    assert!(store
        .knowledge("private:someone-else")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        server.psql("chaosbox", "SELECT body FROM texts"),
        "Preserve explicit operator-selected private scope.\n",
        "statements become embedding inputs"
    );
    let second =
        serde_json::json!({"scope":scope,"records":records,"coverage":"second"}).to_string();
    assert!(store
        .publish_knowledge(scope, &second, &records, None)
        .await
        .is_err());
    let next = store
        .publish_knowledge(scope, &second, &records, Some(&id))
        .await
        .unwrap();
    assert_ne!(next, id);
    assert_eq!(
        store.knowledge_at(scope, &id).await.unwrap(),
        Some(first.clone())
    );
    assert!(store
        .knowledge_at("private:someone-else", &id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .publish_knowledge(scope, &first, &records, Some(&id))
        .await
        .is_err());
    let r = reader(&server).await;
    assert_eq!(
        r.knowledge(scope).await.unwrap(),
        Some((next.clone(), second))
    );
    let mut rival = open(&server).await;
    let left = serde_json::json!({"scope":scope,"records":records,"generation":"left"}).to_string();
    let right =
        serde_json::json!({"scope":scope,"records":records,"generation":"right"}).to_string();
    let (a, b) = tokio::join!(
        store.publish_knowledge(scope, &left, &records, Some(&next)),
        rival.publish_knowledge(scope, &right, &records, Some(&next))
    );
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "exactly one scope publisher may advance"
    );
    assert_eq!(
        store.knowledge(scope).await.unwrap().unwrap().0,
        a.or(b).unwrap()
    );
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_peer_ledger_is_isolated_paginated_collision_checked_and_atomically_published() {
    use chaosbox_store::{ReplicaRow, ReplicaStore};
    let server = Server::start();
    let mut store = open(&server).await;
    let user = "a".repeat(64);
    let scope = "private:peer-test";
    for id in ["1", "2", "3"] {
        let row = ReplicaRow {
            id: id.repeat(64),
            user: user.clone(),
            scope: scope.into(),
            body: format!("event-{id}"),
        };
        store.replica_put(&row).await.unwrap();
        store.replica_put(&row).await.unwrap();
        assert!(store
            .replica_put(&ReplicaRow {
                body: "different bytes".into(),
                ..row
            })
            .await
            .is_err());
    }
    let page = store.replica_rows(&user, scope, "", 2).await.unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(
        store
            .replica_rows(&user, scope, &page[1].id, 2)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .replica_rows(&"b".repeat(64), scope, "", 2)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .replica_rows(&user, "private:other", "", 2)
        .await
        .unwrap()
        .is_empty());
    let first = "4".repeat(64);
    store
        .replica_publish(&user, scope, &first, "first snapshot", None)
        .await
        .unwrap();
    assert_eq!(
        store.replica_current(&user, scope).await.unwrap(),
        Some((first.clone(), "first snapshot".into()))
    );
    assert!(store
        .replica_publish(&user, scope, &first, "collision", Some(&first))
        .await
        .is_err());
    let mut rival = open(&server).await;
    let (left_id, right_id) = ("5".repeat(64), "6".repeat(64));
    let (left, right) = tokio::join!(
        store.replica_publish(&user, scope, &left_id, "left", Some(&first)),
        rival.replica_publish(&user, scope, &right_id, "right", Some(&first))
    );
    assert_ne!(left.is_ok(), right.is_ok(), "one predecessor race winner");
    assert!(store
        .replica_current(&"b".repeat(64), scope)
        .await
        .unwrap()
        .is_none());
    let current = store.replica_current(&user, scope).await.unwrap().unwrap();
    assert_eq!(current.1, if left.is_ok() { "left" } else { "right" });
    assert_eq!(
        open(&server)
            .await
            .replica_current(&user, scope)
            .await
            .unwrap()
            .unwrap(),
        current
    );
}

fn unit(axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; chaosbox_pg::EMBED_DIMS];
    v[axis] = 1.0;
    v
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_embeddings_rank_only_the_pinned_build_and_are_idempotent() {
    let server = Server::start();
    let mut s = open(&server).await;
    let defn = |repo: &str, name: &str, text: &str| {
        let mut e = Entity::new(
            EntityKind::Definition,
            repo,
            "s1",
            "a.rs",
            name,
            &format!("a.rs::{name}"),
            span("a.rs"),
        );
        e.text = Some(text.into());
        e
    };
    let swing = defn("vec", "swing", "fn swing() { /* move the pointer */ }");
    let parse = defn("vec", "parse", "fn parse() {}");
    let bare = ent("vec", "s1", "a.rs", "untexted");
    let mut build = GraphBuild::new("vec", vec!["s1".into()], 1);
    for e in [&swing, &parse, &bare] {
        build.add_node(e.clone()).unwrap();
    }
    s.publish(build.clone(), None).await.unwrap();
    let foreign = defn("other", "swing", "fn swing() { /* move the pointer */ }");
    let mut other = GraphBuild::new("other", vec!["s1".into()], 1);
    other.add_node(foreign).unwrap();
    s.publish(other, None).await.unwrap();
    // Identical inputs across repositories share one text row.
    let pending = s.pending_texts("m@1024", 10).await.unwrap();
    assert_eq!(pending.len(), 2, "{pending:?}");
    assert!(pending
        .iter()
        .any(|(_, body)| body == "a.rs::swing\nfn swing() { /* move the pointer */ }"));
    let vectors: Vec<(String, Vec<f32>)> = pending
        .iter()
        .map(|(sha, body)| (sha.clone(), unit(usize::from(body.contains("parse")))))
        .collect();
    s.put_embeddings("m@1024", &vectors).await.unwrap();
    s.put_embeddings("m@1024", &vectors).await.unwrap();
    assert!(s.pending_texts("m@1024", 10).await.unwrap().is_empty());
    assert_eq!(s.pending_texts("other-model", 10).await.unwrap().len(), 2);
    assert!(s
        .put_embeddings("m@1024", &[(pending[0].0.clone(), vec![1.0; 3])])
        .await
        .is_err());
    let r = reader(&server).await;
    let hits = r
        .nearest_entities(&build.id, "m@1024", &unit(1), 10)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["parse", "swing"],
        "closest first, build-scoped, untexted absent"
    );
    assert!(r
        .nearest_entities(&build.id, "unknown", &unit(1), 10)
        .await
        .unwrap()
        .is_empty());
    let ranked = r
        .rank_texts(
            "m@1024",
            &unit(0),
            &pending
                .iter()
                .map(|(sha, _)| sha.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    assert_eq!(ranked.len(), 2);
    assert!(ranked.iter().any(|(_, score)| (score - 1.0).abs() < 1e-3));
}
