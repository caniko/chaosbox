//! Persistent per-device spending: reserve before dispatch, retain unknown outcomes.
use chaosbox_jev::estimate_tokens;
use chaosbox_store::ReplicaStore;
use crate::Responder;
use super::{
    Identity, Payload, SignedEvent, Resolution, identity, load_replica, persist_event,
    publish_current,
};

/// Cumulative device/scope spending limits, stable across process restarts.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileBudget {
    /// Explicit operator budget epoch. Changing it authorizes a new allowance.
    pub epoch: String,
    /// Total dispatches allowed for this device/epoch, including failed attempts.
    pub requests: u32,
    /// Total conservative reserved input tokens for this device/epoch.
    pub input_tokens: u64,
    /// Bounded number of queued jobs processed during this invocation.
    pub max_jobs: usize,
    /// Explicit permission to retry a failed/interrupted dispatch.
    pub retry: bool,
}

/// Assess pending semantic jobs. Callers serialize dispatch per device, as the
/// CLI does with its process-held device lease. Peer exchange never dispatches.
pub async fn reconcile(
    store: &mut impl ReplicaStore,
    identity: &Identity,
    scope: &str,
    responder: &mut impl Responder,
    budget: &ReconcileBudget,
) -> Result<usize, String> {
    if budget.epoch.trim().is_empty() || budget.epoch.len() > 128 {
        return Err("invalid reconciliation budget epoch".into());
    }
    let replica = load_replica(store, &identity.user(), scope).await?;
    let view = replica.view()?;
    // Failed attempts remain deferred but must not occupy the whole bounded
    // queue forever, starving unrelated new work behind them.
    let excluded = deferred_attempts(&replica, budget.retry);
    let jobs = super::reconcile::eligible_jobs(&replica, &view, budget.max_jobs, &excluded)?;
    let mut completed = 0;
    for job in jobs {
        job.validate()?;
        let replica = load_replica(store, &identity.user(), scope).await?;
        // An exchange may have changed the evidence or resolved this job while
        // earlier jobs were running. Never sign stale inputs against a new frontier.
        if !super::reconcile::eligible_jobs(&replica, &replica.view()?, budget.max_jobs, &excluded)?
            .iter()
            .any(|current| current.id == job.id)
        {
            continue;
        }
        let reservations: Vec<_> = replica
            .events
            .values()
            .filter(|e| e.grant.device == identity.grant.device)
            .filter_map(|e| {
                if let Payload::Reservation {
                    job,
                    budget: epoch,
                    tokens,
                    ..
                } = &e.payload
                {
                    (epoch == &budget.epoch).then_some((job, *tokens))
                } else {
                    None
                }
            })
            .collect();
        if !budget.retry
            && replica
                .events
                .values()
                .any(|e| matches!(&e.payload,Payload::Reservation { job:id,.. } if id == &job.id))
        {
            continue;
        }
        let encoded = serde_json::to_string(&(&job.state, job.questions()))
            .map_err(|_| "encode inference input")?;
        let tokens = u64::try_from(encoded.len().max(estimate_tokens(&encoded)))
            .map_err(|_| "token reservation overflow")?
            .checked_add(1024)
            .ok_or("token reservation overflow")?;
        let used = reservations.iter().try_fold(0u64, |n, (_, t)| {
            n.checked_add(*t).ok_or("token ledger overflow")
        })?;
        if reservations.len() >= budget.requests as usize
            || used
                .checked_add(tokens)
                .is_none_or(|n| n > budget.input_tokens)
        {
            return Err("persistent reconciliation budget exhausted".into());
        }
        let reservation = SignedEvent::signed(
            identity,
            scope,
            replica.heads(),
            Payload::Reservation {
                job: job.id.clone(),
                budget: budget.epoch.clone(),
                tokens,
                attempt: identity::nonce()?,
            },
        )?;
        persist_event(store, &identity.user(), scope, &reservation).await?;
        let response = responder
            .respond(job.state.clone(), job.questions())
            .await
            .map_err(|_| {
                "Jev reconciliation failed; spending reservation retained, retry requires --retry"
            })?;
        let resolution = Resolution::new(job, response)?;
        let mut parents = replica.heads();
        parents.push(reservation.id);
        let event = SignedEvent::resolution(identity, parents, resolution)?;
        persist_event(store, &identity.user(), scope, &event).await?;
        publish_current(store, &identity.user(), scope).await?;
        completed += 1;
    }
    Ok(completed)
}

fn deferred_attempts(replica: &super::Replica, retry: bool) -> std::collections::BTreeSet<String> {
    if retry {
        return std::collections::BTreeSet::new();
    }
    replica
        .events
        .values()
        .filter_map(|e| {
            if let Payload::Reservation { job, .. } = &e.payload {
                Some(job.clone())
            } else {
                None
            }
        })
        .collect()
}
