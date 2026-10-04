//! Durable worker-task lease states and claiming/heartbeat/reclaim logic.

use serde::{Deserialize, Serialize};

use crate::StoreError;

/// Durable worker task states. No transactions held open during Jev calls.
/// Clocks are injected as unix seconds (callers read the real clock);
/// persistence of these records lands with the backend write path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Ready to be claimed by a worker.
    Pending,
    /// Held by a worker under a lease; stale holders never overwrite newer
    /// results, and expired holders become reclaimable.
    Claimed {
        /// Worker holding the claim.
        worker: String,
        /// Unix timestamp when the lease expires.
        expires_at: i64,
    },
    /// Completed.
    Done,
    /// Failed with a sanitized reason.
    Failed(String),
}

/// Durable task record with safe claiming/recovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// Task id.
    pub id: String,
    /// Current lifecycle state.
    pub state: TaskState,
    /// Claim generation; incremented on every successful claim or reclaim.
    pub generation: u64,
}

/// Claim a pending task with a lease: only `Pending` tasks can be claimed,
/// so stale workers never overwrite newer task results.
pub fn claim_task(
    task: &mut Task,
    worker: &str,
    now_unix: i64,
    lease_secs: i64,
) -> Result<(), StoreError> {
    match &task.state {
        TaskState::Pending => {
            task.state = TaskState::Claimed {
                worker: worker.to_owned(),
                expires_at: now_unix + lease_secs.max(1),
            };
            task.generation += 1;
            Ok(())
        }
        other => Err(StoreError::Invariant(format!(
            "claim non-pending task {other:?} as {worker}"
        ))),
    }
}

/// Renew the caller's own lease (heartbeat). Any other holder, or any
/// non-claimed state, is rejected.
pub fn heartbeat_task(
    task: &mut Task,
    worker: &str,
    now_unix: i64,
    lease_secs: i64,
) -> Result<(), StoreError> {
    match &task.state {
        TaskState::Claimed { worker: holder, .. } if holder == worker => {
            task.state = TaskState::Claimed {
                worker: worker.to_owned(),
                expires_at: now_unix + lease_secs.max(1),
            };
            Ok(())
        }
        other => Err(StoreError::Invariant(format!(
            "heartbeat not holder: {other:?} as {worker}"
        ))),
    }
}

/// Reclaim an expired claim for a new worker, bumping the generation so a
/// stale holder's late write is recognizable. Live claims cannot be taken.
pub fn reclaim_task(
    task: &mut Task,
    worker: &str,
    now_unix: i64,
    lease_secs: i64,
) -> Result<(), StoreError> {
    match &task.state {
        TaskState::Claimed { expires_at, .. } if now_unix >= *expires_at => {
            task.state = TaskState::Claimed {
                worker: worker.to_owned(),
                expires_at: now_unix + lease_secs.max(1),
            };
            task.generation += 1;
            Ok(())
        }
        other => Err(StoreError::Invariant(format!(
            "reclaim live task {other:?} as {worker}"
        ))),
    }
}
