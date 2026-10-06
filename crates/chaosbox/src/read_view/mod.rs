//! Run-bound, bounded graph reads behind a trusted connector.
//!
//! The connector authenticates workers, resolves Paperclip policy, confines
//! credentials/transports, and revokes connections. This module enforces the
//! immutable decision; it is not a grants database or an authentication service.

mod contract;
pub mod mcp;
mod query;

use std::{
    collections::BTreeMap,
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use chaosbox_store::{EntityRow, EvidenceRow, GraphQueries, RelRow};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::watch;

pub use contract::{Budgets, Operation, ReadView, RunIdentity};

/// Stable, redacted blocked results. Backend messages never cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// Admission contract cannot be used.
    #[error("invalid read view; request a supported admission from the host connector")]
    InvalidView,
    /// Authenticated caller differs from the admitted run.
    #[error("run identity mismatch; open a separately authorized connection")]
    Identity,
    /// Time-bounded authorization is no longer valid.
    #[error("read view expired or not yet valid; request fresh authorization")]
    Expired,
    /// Connector cancelled/revoked this connection.
    #[error("read view revoked; request fresh authorization")]
    Revoked,
    /// Operation or repository lies outside the decision.
    #[error("operation or repository not granted by this read view")]
    Denied,
    /// Bad types, fields, cursor, or numerical limits.
    #[error("invalid arguments; use the advertised schema and a matching page cursor")]
    Arguments,
    /// A pinned version cannot be loaded exactly.
    #[error(
        "recorded build unavailable; ask the connector to restore that version or start a new conversation"
    )]
    VersionUnavailable,
    /// Complete source scope cannot be proved for returned evidence.
    #[error("complete evidence scope unavailable; request a source-complete authorized build")]
    EvidenceScope,
    /// Bounded work/output exceeded; no partial success.
    #[error("read budget exceeded; narrow the query/page or request a reviewed budget")]
    Budget,
    /// Whole-call deadline exhausted.
    #[error("read deadline exceeded; retry with a narrower query")]
    Deadline,
    /// Infrastructure failure, details kept outside the agent response.
    #[error("graph backend unavailable; ask the host connector to check service readiness")]
    Backend,
}

impl ReadError {
    /// Machine-readable blocked reason.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidView => "invalid_read_view",
            Self::Identity => "identity_mismatch",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::Denied => "denied",
            Self::Arguments => "invalid_arguments",
            Self::VersionUnavailable => "version_unavailable",
            Self::EvidenceScope => "evidence_scope_unavailable",
            Self::Budget => "budget_exceeded",
            Self::Deadline => "deadline_exceeded",
            Self::Backend => "backend_unavailable",
        }
    }
}

/// Connector-held, one-way revocation handle. Clones all revoke the same view.
#[derive(Clone)]
pub struct Revoker(watch::Sender<bool>);

impl Revoker {
    /// Cancel pending reads and reject future reads. Cannot restore a grant.
    pub fn revoke(&self) {
        self.0.send_replace(true);
    }
}

/// One admitted connection. Data identity and scope are immutable and private.
pub struct ScopedReader {
    view: ReadView,
    nodes: BTreeMap<String, EntityRow>,
    edges: BTreeMap<String, RelRow>,
    evidence: BTreeMap<String, Vec<EvidenceRow>>,
    revoker: Revoker,
    calls: AtomicUsize,
}

impl ScopedReader {
    /// Load an exact published version after a trusted connector authorizes it.
    /// All graph reads are bounded before source filtering. No producer API is used.
    pub async fn admit<R: GraphQueries>(
        handle: R,
        view: ReadView,
        identity: &RunIdentity,
    ) -> Result<Self, ReadError> {
        let started = tokio::time::Instant::now();
        view.validate_at(identity, unix_now()?)?;
        let deadline = Duration::from_millis(view.budgets.timeout_ms);
        let result = tokio::time::timeout_at(started + deadline, Self::load(handle, view))
            .await
            .map_err(|_| ReadError::Deadline)??;
        result.check(identity)?;
        if started.elapsed() >= deadline {
            return Err(ReadError::Deadline);
        }
        Ok(result)
    }

    async fn load<R: GraphQueries>(handle: R, view: ReadView) -> Result<Self, ReadError> {
        let header = handle
            .published_build(&view.repo, &view.build_id)
            .await
            .map_err(|_| ReadError::Backend)?
            .ok_or(ReadError::VersionUnavailable)?;
        if header.build_id != view.build_id
            || header.status != "active"
            || !header.evidence_sealed
            || header
                .snapshots
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != view.snapshots
        {
            return Err(ReadError::VersionUnavailable);
        }
        let nodes = handle
            .build_entities(&view.build_id, limit_plus_one(view.budgets.max_nodes))
            .await
            .map_err(|_| ReadError::Backend)?;
        let edges = handle
            .build_relationships(&view.build_id, limit_plus_one(view.budgets.max_edges))
            .await
            .map_err(|_| ReadError::Backend)?;
        if nodes.len() > view.budgets.max_nodes || edges.len() > view.budgets.max_edges {
            return Err(ReadError::Budget);
        }
        let mut bytes = bounded_json(&(&nodes, &edges), view.budgets.max_graph_bytes)?.len();
        let all_ids: std::collections::BTreeSet<_> =
            nodes.iter().map(|n| n.entity_id.as_str()).collect();
        if all_ids.len() != nodes.len()
            || nodes.iter().any(|n| {
                n.repo != view.repo
                    || !view.snapshots.contains(&n.snapshot)
                    || !contract::valid_path(&n.file)
                    || n.span.as_ref().is_some_and(|span| span.file != n.file)
            })
            || edges.iter().any(|e| {
                !all_ids.contains(e.from_entity.entity_id.as_str())
                    || !all_ids.contains(e.to_entity.entity_id.as_str())
            })
        {
            return Err(ReadError::EvidenceScope);
        }
        let nodes: BTreeMap<_, _> = nodes
            .into_iter()
            .filter(|n| view.allows_file(&n.file))
            .map(|n| (n.entity_id.clone(), n))
            .collect();
        let edges: BTreeMap<_, _> = edges
            .into_iter()
            .filter(|e| {
                nodes.contains_key(&e.from_entity.entity_id)
                    && nodes.contains_key(&e.to_entity.entity_id)
            })
            .map(|e| (e.rel_id.clone(), e))
            .collect();
        // Authorize the complete transitive evidence before revealing even a
        // relationship's existence (neighbors, paths, counts and export too).
        // Freeze evidence with graph members; subsequent calls need no backend.
        let mut evidence = BTreeMap::new();
        for id in edges.keys() {
            tokio::task::yield_now().await;
            let rows = handle
                .evidence_for_limited(
                    &view.build_id,
                    id,
                    limit_plus_one(view.budgets.max_evidence),
                )
                .await
                .map_err(|error| match error {
                    chaosbox_store::StoreError::QueryBudget => ReadError::Budget,
                    chaosbox_store::StoreError::EvidenceClosureUnavailable => {
                        ReadError::EvidenceScope
                    }
                    _ => ReadError::Backend,
                })?;
            validate_evidence(&view, &rows)?;
            bytes += bounded_json(&rows, view.budgets.max_graph_bytes.saturating_sub(bytes))?.len();
            evidence.insert(id.clone(), rows);
        }
        let (tx, _) = watch::channel(false);
        Ok(Self {
            view,
            nodes,
            edges,
            evidence,
            revoker: Revoker(tx),
            calls: AtomicUsize::new(0),
        })
    }

    /// Immutable decision for connector bookkeeping; never accept a replacement.
    #[must_use]
    pub fn view(&self) -> &ReadView {
        &self.view
    }

    /// One-way control handle for policy revocation, cancellation and cleanup.
    #[must_use]
    pub fn revoker(&self) -> Revoker {
        self.revoker.clone()
    }

    fn check(&self, identity: &RunIdentity) -> Result<(), ReadError> {
        if *self.revoker.0.borrow() {
            return Err(ReadError::Revoked);
        }
        self.view.validate_at(identity, unix_now()?)
    }

    /// Execute strictly typed tool arguments for the connector-authenticated run.
    /// Every authenticated attempt consumes the connection budget. Callers must
    /// also cancel the transport on run completion.
    pub async fn call(
        &self,
        identity: &RunIdentity,
        name: &str,
        args: Value,
    ) -> Result<Value, ReadError> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.view.budgets.timeout_ms);
        self.call_until(identity, name, args, deadline).await
    }

    // The transport captures this deadline when it accepts a request, before
    // scheduling the task, and also applies it to response delivery.
    async fn call_until(
        &self,
        identity: &RunIdentity,
        name: &str,
        args: Value,
        deadline: tokio::time::Instant,
    ) -> Result<Value, ReadError> {
        self.check(identity)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(ReadError::Deadline);
        }
        self.calls
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < self.view.budgets.max_calls).then(|| n + 1)
            })
            .map_err(|_| ReadError::Budget)?;
        bounded_json(&args, self.view.budgets.max_request_bytes)?;
        let request = query::Request::parse(name, args)?;
        if request.repo() != self.view.repo || !self.view.operations.contains(&request.operation())
        {
            return Err(ReadError::Denied);
        }
        let mut cancelled = self.revoker.0.subscribe();
        let work = async {
            let data = self.query(request).await?;
            let result = json!({
                "version": 1,
                "read_view": {"id":self.view.id, "policy_revision":self.view.policy_revision,
                    "identity":self.view.identity, "repo":self.view.repo, "build_id":self.view.build_id,
                    "snapshots":self.view.snapshots},
                "workspace_observation": null, "applicability":"unknown", "data": data
            });
            bounded_json(&result, self.view.budgets.max_response_bytes)?;
            self.check(identity)?;
            if tokio::time::Instant::now() >= deadline {
                return Err(ReadError::Deadline);
            }
            Ok(result)
        };
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|revoked| *revoked) => Err(ReadError::Revoked),
            result = tokio::time::timeout_at(deadline, work) => result.map_err(|_| ReadError::Deadline)?,
        }
    }
}

fn validate_evidence(view: &ReadView, rows: &[EvidenceRow]) -> Result<(), ReadError> {
    if rows.len() > view.budgets.max_evidence {
        return Err(ReadError::Budget);
    }
    if rows.is_empty() {
        return Err(ReadError::EvidenceScope);
    }
    for row in rows {
        let Some(source) = &row.citation else {
            return Err(ReadError::EvidenceScope);
        };
        if !view.snapshots.contains(&source.snapshot)
            || !view.allows_file(&source.file)
            || source.sha256.len() != 64
            || !source.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || source
                .span
                .as_ref()
                .is_some_and(|span| span.file != source.file)
        {
            return Err(ReadError::EvidenceScope);
        }
    }
    Ok(())
}

pub(super) fn limit_plus_one(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX - 1) + 1
}

pub(super) fn unix_now() -> Result<u64, ReadError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| ReadError::Expired)
}

/// Serialize with an allocation bound rather than allocating then checking size.
pub(super) fn bounded_json<T: Serialize>(value: &T, cap: usize) -> Result<Vec<u8>, ReadError> {
    struct Buffer {
        bytes: Vec<u8>,
        cap: usize,
    }
    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.len() > self.cap.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("JSON byte budget exceeded"));
            }
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Buffer {
        bytes: Vec::new(),
        cap,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| ReadError::Budget)?;
    Ok(writer.bytes)
}
