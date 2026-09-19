//! Live TypeDB backend tests: full write/publish/readback cycle, decision
//! supersedure, predecessor guards, and concurrent-publication races against
//! a real server.
//!
//! Requires a reachable TypeDB server: address from `TYPEDB_ADDR`
//! (default `127.0.0.1:1729`). Without one the tests report a skip and pass;
//! a skip is NOT conformance evidence (see the execution ledger). The named
//! CI gate runs these with a server present.

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, EntityKind, Evidence, EvidenceClass,
    GraphBuild, Relation, RelationScope, RelationType, SourceSpan,
};
use chaosbox_gel::Store;
use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};

fn addr() -> String {
    std::env::var("TYPEDB_ADDR").unwrap_or_else(|_| "127.0.0.1:1729".into())
}

fn config(db: &str) -> TypeDbConfig {
    TypeDbConfig {
        address: addr(),
        username: "admin".into(),
        password: "password".into(),
        database: db.into(),
    }
}

async fn connected_store(db: &str) -> Option<TypeDbStore> {
    let mut s = TypeDbStore::new(config(db));
    match s.migrate().await {
        Ok(()) => Some(s),
        Err(e) => {
            println!("SKIP (no server at {}): {e}", addr());
            None
        }
    }
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
    }
}

fn candidate(id: &str, from: &str, to: &str) -> Candidate {
    Candidate {
        id: id.into(),
        rel_type: RelationType::Calls,
        from_entity: from.into(),
        to_entity: to.into(),
        reason: "structural".into(),
        state_excerpt: "fn a() {}".into(),
    }
}

async fn seed_files_run(
    s: &mut TypeDbStore,
    repo: &str,
    snap: &str,
) -> (String, String, String) {
    use chaosbox_core::SnapshotFile;
    let files = vec![SnapshotFile {
        snapshot: snap.into(),
        path: "a.rs".into(),
        sha256: "abc".into(),
        bytes: 10,
    }];
    s.ensure_snapshot_files(snap, repo, &files).await.unwrap();
    let run = format!("run-{snap}");
    let set = format!("set-{snap}");
    s.ensure_run(&run, repo, snap, &set, "digest-1", "rubric-v1")
        .await
        .unwrap();
    (run, set, snap.into())
}

#[tokio::test]
async fn migrate_is_idempotent() {
    let Some(mut s) = connected_store("t_migrate").await else {
        return;
    };
    s.migrate().await.unwrap();
    s.migrate().await.unwrap();
}

#[tokio::test]
async fn publish_readback_and_predecessor_guards() {
    let Some(mut s) = connected_store("t_pub").await else {
        return;
    };
    let repo = "pubrepo";
    let (_run, set, _snap) = seed_files_run(&mut s, repo, "s1").await;

    let a = ent(repo, "s1", "a.rs", "a");
    let b = ent(repo, "s1", "a.rs", "b");
    s.put_candidate(&set, &candidate("cand:1", &a.id, &b.id)).await.unwrap();
    s.put_decision(decision("cand:1", "q1", "key-1")).await.unwrap();
    s.put_evidence(Evidence {
        id: "ev:1".into(),
        class: EvidenceClass::Extracted,
        supports: true,
        text: "fn a() {}".into(),
        span: Some(span("a.rs")),
        snapshot: "s1".into(),
        source_file_version: "a.rs".into(),
    })
    .await
    .unwrap();
    s.put_claim(Claim {
        id: "claim:1".into(),
        relation_id: "rel:pending".into(),
        supporting: vec!["ev:1".into()],
        contradicting: vec![],
        accepted: true,
    })
    .await
    .unwrap();

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

    // Live readback through a FRESH store (nothing staged): decisions,
    // evidence linkage and the pointer swing really landed.
    let mut fresh = TypeDbStore::new(config("t_pub"));
    fresh.migrate().await.unwrap();
    let found = fresh.find_decision("cand:1", "q1").await.unwrap().unwrap();
    assert_eq!(found.cache_key, "key-1");
    assert_eq!(found.id, "dec:cand:1:q1");

    // Second generation with the predecessor wins.
    let mut build2 = GraphBuild::new(repo, vec!["s1".into()], 2);
    build2.predecessor = Some(bid.clone());
    build2.add_node(a.clone()).unwrap();
    build2.add_node(b.clone()).unwrap();
    s.publish(build2.clone(), Some(bid.clone())).await.unwrap();

    // Stale predecessor and older generation both fail; last good stands.
    let mut stale = GraphBuild::new(repo, vec!["s1".into()], 3);
    stale.add_node(a.clone()).unwrap();
    let err = s.publish(stale, Some("build:stale".into())).await.unwrap_err();
    assert!(err.to_string().contains("predecessor mismatch"), "{err}");
    let mut older = GraphBuild::new(repo, vec!["s1".into()], 1);
    older.predecessor = Some(build2.id.clone());
    older.add_node(a.clone()).unwrap();
    let err = s.publish(older, Some(build2.id.clone())).await.unwrap_err();
    assert!(err.to_string().contains("older worker"), "{err}");
}

#[tokio::test]
async fn concurrent_publishers_from_same_generation_exactly_one_wins() {
    let Some(mut s1) = connected_store("t_race").await else {
        return;
    };
    let repo = "racerepo";
    seed_files_run(&mut s1, repo, "s1").await;
    seed_files_run(&mut s1, repo, "s2").await;
    let a = ent(repo, "s1", "a.rs", "a");
    let mut gen1 = GraphBuild::new(repo, vec!["s1".into()], 1);
    gen1.add_node(a.clone()).unwrap();
    s1.publish(gen1.clone(), None).await.unwrap();

    // Same build published twice is idempotent, not a conflict.
    s1.publish(gen1.clone(), Some(gen1.id.clone())).await.unwrap();
    let mut retry = TypeDbStore::new(config("t_race"));
    retry.migrate().await.unwrap();
    retry.publish(gen1.clone(), None).await.unwrap();

    // Genuine race: two distinct generation-2 builds from the same
    // predecessor, published concurrently. Exactly one wins; the loser
    // reports a conflict and the pointer holds the winner.
    let mut b1 = GraphBuild::new(repo, vec!["s1".into()], 2);
    b1.predecessor = Some(gen1.id.clone());
    b1.add_node(a.clone()).unwrap();
    let mut b2 = GraphBuild::new(repo, vec!["s2".into()], 2);
    b2.predecessor = Some(gen1.id.clone());
    b2.add_node(a.clone()).unwrap();
    assert_ne!(b1.id, b2.id, "rival builds must differ");
    let mut w1 = TypeDbStore::new(config("t_race"));
    w1.migrate().await.unwrap();
    let mut w2 = TypeDbStore::new(config("t_race"));
    w2.migrate().await.unwrap();
    let (r1, r2) = tokio::join!(
        w1.publish(b1.clone(), Some(gen1.id.clone())),
        w2.publish(b2.clone(), Some(gen1.id.clone()))
    );
    assert!(r1.is_ok() ^ r2.is_ok(), "exactly one publisher wins: {r1:?} vs {r2:?}");

    // The loser retrying with its stale predecessor fails without moving
    // the pointer; last good build stays active.
    let mut late = TypeDbStore::new(config("t_race"));
    late.migrate().await.unwrap();
    let stale = if r1.is_ok() { b2 } else { b1 };
    let err = late.publish(stale, Some(gen1.id.clone())).await.unwrap_err();
    assert!(
        err.to_string().contains("predecessor mismatch")
            || err.to_string().contains("concurrent publisher"),
        "{err}"
    );
}
