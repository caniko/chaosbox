//! Shared persistence abstractions for Chaosbox storage backends.
//!
//! [`Store`] is the write-side contract (idempotent staging, invariant
//! validation, guarded publication); [`GraphQueries`] is the read-side
//! contract (every read scoped to one pinned active build). Both ship with
//! in-memory implementations — [`MemoryStore`] and [`MemoryReader`] — for
//! tests and environments without a server, and `chaosbox-typedb`
//! implements them over the live `TypeDB` backend. The conformance suite
//! ([`check_conformance`]) proves backend read parity; the write-path
//! suite lives in this crate's tests.

use thiserror::Error;

mod conformance;
mod memory_reader;
mod queries;
mod rows;
mod store;
mod task;

pub use conformance::{ConformanceSeed, check_conformance, conformance_seed};
pub use memory_reader::MemoryReader;
pub use queries::GraphQueries;
pub use rows::{BuildRow, EndpointRef, EntityRow, EvidenceRow, RelRow};
pub use store::{MemoryStore, StagedData, Store, StoreStats};
pub use task::{Task, TaskState, claim_task, heartbeat_task, reclaim_task};

/// Persistence/query failures: connection, query, invariant, or missing rows.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("connection: {0}")]
    /// Connection or client-construction failure.
    Connection(String),
    #[error("query: {0}")]
    /// Query execution or typed-decoding failure.
    Query(String),
    #[error("invariant: {0}")]
    /// A graph invariant was violated (cross-build edge, stale predecessor, ...).
    Invariant(String),
    #[error("not found: {0}")]
    /// A required row (build, entity, ...) does not exist.
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaosbox_core::{
        Candidate, Decision, Entity, EntityKind, Evidence, GraphBuild, SnapshotFile, SourceSpan,
    };

    fn ent(repo: &str, snap: &str, file: &str, name: &str) -> Entity {
        Entity::new(
            EntityKind::Symbol,
            repo,
            snap,
            file,
            name,
            name,
            SourceSpan::point(file, 1, 1, 0),
        )
    }

    /// Shared write-path conformance over any [`Store`] impl: file linkage,
    /// decision idempotency + Failed-supersedure, and publication guards.
    /// Runs against [`MemoryStore`] now; a live-backend test seeds nothing
    /// extra and calls this against the `TypeDB` store once available.
    pub async fn check_write_conformance<S: Store>(s: &mut S) {
        use chaosbox_core::{DecisionOutcome, EvidenceClass, RelationType, SourceSpan};
        // Run + set identity registers before candidates may reference it.
        s.ensure_run("run:1", "r", "s1", "set:1", "catalog:1", "rubric-v1")
            .await
            .unwrap();
        assert_eq!(s.stats().runs, 1);
        // Same inputs re-register idempotently; changed inputs are rejected.
        s.ensure_run("run:1", "r", "s1", "set:1", "catalog:1", "rubric-v1")
            .await
            .unwrap();
        assert!(s
            .ensure_run("run:1", "r", "s1", "set:1", "catalog:2", "rubric-v1")
            .await
            .is_err());
        let cand = Candidate {
            id: "cand:1".into(),
            rel_type: RelationType::Calls,
            from_entity: "ent:a".into(),
            to_entity: "ent:b".into(),
            reason: "structural".into(),
            state_excerpt: String::new(),
        };
        assert!(
            s.put_candidate("set:missing", &cand).await.is_err(),
            "unregistered sets never resolve"
        );
        s.put_candidate("set:1", &cand).await.unwrap();
        s.put_candidate("set:1", &cand).await.unwrap();
        assert_eq!(s.stats().candidates, 1);
        // File versions register before evidence may reference them.
        let files = vec![SnapshotFile {
            snapshot: "s1".into(),
            path: "a.rs".into(),
            sha256: "abc".into(),
            bytes: 3,
        }];
        s.ensure_snapshot_files("s1", "r", &files).await.unwrap();
        let ev = Evidence {
            id: "ev1".into(),
            class: EvidenceClass::Extracted,
            supports: true,
            text: "[structural] a".into(),
            span: Some(SourceSpan::point("a.rs", 1, 1, 0)),
            snapshot: "s1".into(),
            source_file_version: "a.rs".into(),
        };
        s.put_evidence(ev).await.unwrap();
        assert_eq!(s.stats().evidence, 1);
        let bad = Evidence {
            id: "ev2".into(),
            class: EvidenceClass::Ambiguous,
            supports: false,
            text: "x".into(),
            span: None,
            snapshot: "s9".into(),
            source_file_version: "missing.rs".into(),
        };
        assert!(
            s.put_evidence(bad).await.is_err(),
            "unregistered files never resolve"
        );
        // Decisions: first write wins, except Failed supersedes once.
        let mk = |id: &str, outcome| Decision {
            id: id.into(),
            candidate_id: "c1".into(),
            question_id: "q1".into(),
            outcome,
            evidence_class: EvidenceClass::Ambiguous,
            model_requested: "jev-1.13.0".into(),
            model_returned: "jev-1.13.0".into(),
            confidence: None,
            probability: None,
            cache_key: "test-cache-key".into(),
        };
        s.put_decision(mk("d1", DecisionOutcome::Failed("down".into())))
            .await
            .unwrap();
        s.put_decision(mk("d2", DecisionOutcome::Accepted))
            .await
            .unwrap();
        assert_eq!(s.stats().decisions, 1, "one row per (candidate, question)");
        s.put_decision(mk("d3", DecisionOutcome::Rejected))
            .await
            .unwrap();
        assert_eq!(s.stats().decisions, 1, "accepted outcomes stick");
        // Publication: stale predecessors and older generations rejected.
        let mut b1 = GraphBuild::new("r", vec!["s1".into()], 1);
        b1.add_node(ent("r", "s1", "a.rs", "a")).unwrap();
        s.publish(b1.clone(), None).await.unwrap();
        let mut stale = GraphBuild::new("r", vec!["s1".into()], 1);
        stale.add_node(ent("r", "s1", "b.rs", "b")).unwrap();
        assert!(s
            .publish(stale, Some("wrong-predecessor".into()))
            .await
            .is_err());
        let mut b2 = GraphBuild::new("r", vec!["s2".into()], 2);
        b2.predecessor = Some(b1.id.clone());
        b2.add_node(ent("r", "s2", "c.rs", "c")).unwrap();
        assert!(s.publish(b2.clone(), Some(b1.id.clone())).await.is_ok());
        assert_eq!(s.active("r").unwrap().id, b2.id);
        assert_eq!(s.stats().builds, 2);
    }

    #[tokio::test]
    async fn memory_store_write_conformance() {
        let mut s = MemoryStore::new();
        check_write_conformance(&mut s).await;
    }

    #[test]
    fn task_claim_recovery() {
        let mut t = Task {
            id: "t".into(),
            state: TaskState::Pending,
            generation: 0,
        };
        claim_task(&mut t, "w1", 1_000, 60).unwrap();
        assert!(
            claim_task(&mut t, "w2", 1_001, 60).is_err(),
            "stale worker must not overwrite"
        );
        // Heartbeats renew the holder's own lease; others are rejected.
        heartbeat_task(&mut t, "w1", 1_050, 60).unwrap();
        assert!(heartbeat_task(&mut t, "w2", 1_050, 60).is_err());
        // Live claims cannot be reclaimed, even by a third worker.
        assert!(reclaim_task(&mut t, "w3", 1_051, 60).is_err());
        // After expiry the task is reclaimable with a bumped generation.
        reclaim_task(&mut t, "w3", 2_000, 60).unwrap();
        assert_eq!(t.generation, 2);
        assert!(matches!(&t.state, TaskState::Claimed { worker, .. } if worker == "w3"));
        // The stale holder's heartbeat now fails: its write is recognizable.
        assert!(heartbeat_task(&mut t, "w1", 2_001, 60).is_err());
    }

    #[tokio::test]
    async fn memory_reader_passes_conformance() {
        let seed = conformance_seed();
        check_conformance(
            &seed.reader,
            &seed.a1,
            &seed.b1,
            &seed.rel1,
            &seed.a2,
            &seed.builds,
        )
        .await;
    }
}
