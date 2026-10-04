//! Same-user peer replication of intelligence and bounded evidence capsules.
//! Events are immutable; a canonical projection is independent of delivery order.
pub mod cli;
pub mod current;
mod identity;
mod persistence;
mod projection;
mod reconcile;
mod settings;
mod sources;
pub mod transport;
mod worker;
pub use identity::{Authority, Grant, Identity};
pub use projection::View;
pub use persistence::{load_replica, persist_event, publish_current};
pub use reconcile::{Action, Job, Resolution, reconcile_jobs};
pub use sources::publication_from_sources;
pub use worker::{ReconcileBudget, reconcile};

use std::collections::BTreeMap;
use chaosbox_core::sha256_hex;
use chaosbox_core::intelligence::IntelligenceCandidate;
use serde::{Deserialize, Serialize};
use crate::intelligence::Bundle;

/// Per-event byte ceiling; scopes are bounded independently of wire pagination.
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum retained events in one explicitly partitioned scope.
pub const MAX_EVENTS: usize = 10_000;
/// Aggregate materialization budget, with explicit refusal instead of truncation.
pub const MAX_REPLICA_BYTES: usize = 64 * 1024 * 1024;

/// Source-verified intelligence plus the exact candidates behind its receipts.
/// The signed publisher attests source capture; peers verify capsule integrity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    /// Validated same-scope intelligence and full typed receipts.
    pub bundle: Bundle,
    /// Bounded original evidence/context, never a complete session archive.
    pub candidates: Vec<IntelligenceCandidate>,
}

impl Publication {
    /// Verify capsule bindings before it enters a replica.
    pub fn validate(&self, scope: &str) -> Result<(), String> {
        self.bundle.validate()?;
        if self.bundle.scope != scope {
            return Err("publication scope mismatch".into());
        }
        let candidates: BTreeMap<_, _> = self.candidates.iter().map(|c| (&c.id, c)).collect();
        if candidates.len() != self.candidates.len() {
            return Err("duplicate source capsule".into());
        }
        let referenced: std::collections::BTreeSet<_> = self
            .bundle
            .assessments
            .iter()
            .map(|a| &a.candidate_id)
            .collect();
        if candidates
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            != referenced
        {
            return Err("source capsules must match retained receipt inputs".into());
        }
        for receipt in &self.bundle.assessments {
            let c = candidates
                .get(&receipt.candidate_id)
                .ok_or("missing source capsule for receipt")?;
            crate::intelligence::validate_replication_receipt(c, receipt, &self.bundle)?;
        }
        validate_metadata(&self.bundle)?;
        Ok(())
    }
}

fn validate_metadata(bundle: &Bundle) -> Result<(), String> {
    use crate::intelligence::Outcome;
    for record in &bundle.records {
        let primary = bundle
            .assessments
            .iter()
            .find(|a| record.assessments.first() == Some(&a.id))
            .ok_or("missing original admission")?;
        if record.kind != crate::intelligence::replication_kind(primary)? {
            return Err("classification does not match the original admission".into());
        }
        let mut contradictions = std::collections::BTreeSet::new();
        let mut supersedes = None;
        for receipt in &bundle.assessments {
            let origin = format!("intel:{}", sha256_hex(&[&receipt.candidate_id]));
            match &receipt.outcome {
                Outcome::Contradiction(other) if origin == record.id => {
                    contradictions.insert(other.clone());
                }
                Outcome::Contradiction(other) if other == &record.id => {
                    contradictions.insert(origin);
                }
                Outcome::Supersession(other) if origin == record.id => {
                    if supersedes.as_ref().is_some_and(|old| old != other) {
                        return Err("ambiguous supersession metadata".into());
                    }
                    supersedes = Some(other.clone());
                }
                _ => {}
            }
        }
        if record
            .contradicts
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            != contradictions
            || record.supersedes != supersedes
        {
            return Err("relationships do not replay from retained receipts".into());
        }
    }
    Ok(())
}

/// An immutable update payload. Semantic decisions are stored separately from sources.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
    /// Import an explicitly source-verified publication.
    Publication(Publication),
    /// Typed, input-bound semantic reconciliation; it never supplies source evidence.
    Resolution(Resolution),
    /// Durable pre-dispatch spending reservation. Unknown outcomes retain their charge.
    Reservation {
        /// Job identity; matching inputs are not re-asked without explicit retry.
        job: String,
        /// Operator-selected budget epoch, persistent across restarts.
        budget: String,
        /// Conservative reserved token ceiling for this dispatch.
        tokens: u64,
        /// Unique dispatch identity, including explicit retries.
        attempt: String,
    },
}

/// A content-addressed, signed causal update, safe to relay through another peer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEvent {
    /// Protocol version, never inferred from the payload shape.
    pub version: u32,
    /// Full event identity including author, scope and causal parents.
    pub id: String,
    /// Enrollment proof of the original author, not the relaying host.
    pub grant: Grant,
    /// Exact private visibility boundary.
    pub scope: String,
    /// Sorted causal parents; missing parents remain staged.
    pub parents: Vec<String>,
    /// Immutable intelligence/evidence content.
    pub payload: Payload,
    /// Device signature over the content identity.
    pub signature: String,
}

impl SignedEvent {
    fn identity(&self) -> Result<String, String> {
        let text = serde_json::to_string(&(
            self.version,
            &self.grant,
            &self.scope,
            &self.parents,
            &self.payload,
        ))
        .map_err(|_| "encode event")?;
        Ok(sha256_hex(&[&text]))
    }
    /// Sign a publication after checking its evidence and receipts.
    pub fn publication(
        identity: &Identity,
        parents: Vec<String>,
        publication: Publication,
    ) -> Result<Self, String> {
        let scope = publication.bundle.scope.clone();
        Self::signed(identity, &scope, parents, Payload::Publication(publication))
    }
    /// Replicate a validated semantic receipt with the source frontier it considered.
    pub fn resolution(
        identity: &Identity,
        parents: Vec<String>,
        resolution: Resolution,
    ) -> Result<Self, String> {
        let scope = resolution.job.scope.clone();
        Self::signed(identity, &scope, parents, Payload::Resolution(resolution))
    }
    pub(crate) fn signed(
        identity: &Identity,
        scope: &str,
        mut parents: Vec<String>,
        payload: Payload,
    ) -> Result<Self, String> {
        parents.sort();
        parents.dedup();
        let mut event = Self {
            version: 1,
            id: String::new(),
            grant: identity.grant.clone(),
            scope: scope.into(),
            parents,
            payload,
            signature: String::new(),
        };
        event.id = event.identity()?;
        event.signature = identity.sign("chaosbox-event-v1\0", event.id.as_bytes());
        event.validate(&identity.user(), scope)?;
        Ok(event)
    }
    /// Validate version, membership, identity, signature and payload bindings.
    pub fn validate(&self, user: &str, scope: &str) -> Result<(), String> {
        self.grant.validate(user, scope)?;
        if self.version != 1
            || self.scope != scope
            || self.id != self.identity()?
            || self.parents.len() > MAX_EVENTS
            || self.parents.windows(2).any(|w| w[0] >= w[1])
            || self.parents.iter().any(|p| p == &self.id || !is_digest(p))
            || serde_json::to_vec(self).map_err(|_| "encode event")?.len() > MAX_EVENT_BYTES
        {
            return Err("invalid or oversized replication event".into());
        }
        identity::verify(
            &self.grant.device,
            "chaosbox-event-v1\0",
            self.id.as_bytes(),
            &self.signature,
        )?;
        match &self.payload {
            Payload::Publication(p) => p.validate(scope),
            Payload::Resolution(r) if r.job.scope == scope => r.validate(),
            Payload::Reservation {
                job,
                budget,
                tokens,
                attempt,
            } if is_digest(job)
                && is_digest(attempt)
                && !budget.trim().is_empty()
                && budget.len() <= 128
                && *tokens > 0 =>
            {
                Ok(())
            }
            _ => Err("invalid replication payload scope or reservation".into()),
        }
    }
}

pub(crate) fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A user/scope-isolated event set. Insertion is commutative and idempotent.
#[derive(Clone)]
pub struct Replica {
    /// Stable root-public-key identity.
    pub user: String,
    /// Exact visibility scope.
    pub scope: String,
    /// Authenticated history, including staged updates with missing parents.
    pub(crate) events: BTreeMap<String, SignedEvent>,
    bytes: usize,
}

impl Replica {
    /// Empty isolated replica; no database or inference side effects.
    #[must_use]
    pub fn new(user: &str, scope: &str) -> Self {
        Self {
            user: user.into(),
            scope: scope.into(),
            events: BTreeMap::new(),
            bytes: 0,
        }
    }
    /// Authenticated immutable history, including staged missing-parent events.
    #[must_use]
    pub fn events(&self) -> &BTreeMap<String, SignedEvent> {
        &self.events
    }
    /// Accept one authenticated event, refusing capacity overruns explicitly.
    pub fn receive(&mut self, event: SignedEvent) -> Result<(), String> {
        event.validate(&self.user, &self.scope)?;
        if let Some(old) = self.events.get(&event.id) {
            if serde_json::to_vec(old).map_err(|_| "encode event")?
                != serde_json::to_vec(&event).map_err(|_| "encode event")?
            {
                return Err("event identity collision".into());
            }
            return Ok(());
        }
        if self.events.len() == MAX_EVENTS {
            return Err("replica event ceiling; partition the scope".into());
        }
        let bytes = self
            .bytes
            .checked_add(
                serde_json::to_vec(&event)
                    .map_err(|_| "encode event")?
                    .len(),
            )
            .ok_or("replica byte overflow")?;
        if bytes > MAX_REPLICA_BYTES {
            return Err("replica byte ceiling; partition the scope".into());
        }
        self.bytes = bytes;
        self.events.insert(event.id.clone(), event);
        Ok(())
    }
    /// Current causal frontier; generations on different hosts are not compared.
    #[must_use]
    pub fn heads(&self) -> Vec<String> {
        let parents: std::collections::BTreeSet<_> =
            self.events.values().flat_map(|e| &e.parents).collect();
        self.events
            .keys()
            .filter(|id| !parents.contains(id))
            .cloned()
            .collect()
    }
    /// Cross-host content identity of this owner's complete immutable event set.
    pub fn digest(&self) -> Result<String, String> {
        Ok(sha256_hex(&[
            &self.user,
            &self.scope,
            &serde_json::to_string(&self.events.keys().collect::<Vec<_>>())
                .map_err(|_| "encode frontier")?,
        ]))
    }
    /// Materialize only a complete, acyclic authenticated history.
    pub fn view(&self) -> Result<View, String> {
        projection::project(self)
    }
}
