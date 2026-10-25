//! Shared persistence abstractions for Chaosbox storage backends.
//!
//! [`Store`] is the write-side contract (idempotent staging, invariant
//! validation, guarded publication); [`GraphQueries`] is the read-side
//! contract (member reads scoped to one explicit build). Both ship with
//! in-memory implementations — [`MemoryStore`] and [`MemoryReader`] — for
//! tests and environments without a server, and `chaosbox-pg`
//! implements them over PostgreSQL. The conformance suites
//! ([`check_conformance`], [`check_write_conformance`]) prove backend
//! read and write parity.

use thiserror::Error;

mod conformance;
mod memory_reader;
mod queries;
mod replica;
mod rows;
mod sealed_evidence;
mod store;
mod task;

pub use conformance::{ConformanceSeed, check_conformance, check_write_conformance, conformance_seed};
pub use memory_reader::MemoryReader;
pub use queries::{GraphQueries, NeighborhoodDirection};
pub use replica::{MemoryReplicaStore, ReplicaRow, ReplicaStore, validate_row, validate_replica_view};
pub use rows::{BuildRow, EndpointRef, EntityRow, EvidenceRow, RelRow, SourceCitation};
pub use sealed_evidence::SEALED_EVIDENCE_BYTES_MAX;
pub use store::{MemoryStore, StagedData, StagedDataRef, Store, StoreStats};
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
    #[error("query row budget exceeded; completeness cannot be proved")]
    /// A bounded backend query saturated before a complete result was known.
    QueryBudget,
    #[error("published evidence closure unavailable; republish under a new build identity")]
    /// A legacy membership has no publication-sealed, source-complete evidence.
    EvidenceClosureUnavailable,
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
