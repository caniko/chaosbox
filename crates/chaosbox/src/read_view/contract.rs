//! Strict connector-owned admission data. None of these fields authenticate a caller.

use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};

use super::ReadError;

/// Authenticated execution identity supplied by the trusted host connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIdentity {
    /// Paperclip company.
    pub company: String,
    /// Authorized project.
    pub project: String,
    /// Executing agent.
    pub agent: String,
    /// Assigned task.
    pub task: String,
    /// Admitted run, not a persistent agent/session id.
    pub run: String,
    /// Execution host (not the controller host by default).
    pub host: String,
    /// Host-local server registration.
    pub server: String,
}

/// Closed read-only operation vocabulary for contract v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Pinned status, never current workspace proof.
    Status,
    /// Literal name search.
    Search,
    /// Entity lookup.
    Lookup,
    /// Directed neighborhood rows.
    Neighbors,
    /// Bounded undirected reachability path.
    Path,
    /// Source-linked entity metadata.
    Explain,
    /// Fully source-authorized relationship evidence.
    Evidence,
    /// Paged node or edge export.
    Export,
}

/// Connector-selected budgets, all bounded by implementation ceilings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    /// Maximum nodes loaded at admission, including filtered-out nodes.
    pub max_nodes: usize,
    /// Maximum edges loaded at admission.
    pub max_edges: usize,
    /// Maximum serialized graph bytes at admission.
    pub max_graph_bytes: usize,
    /// Evidence rows per relationship; excess blocks, never truncates evidence.
    pub max_evidence: usize,
    /// Maximum items per list response.
    pub max_page_size: usize,
    /// Maximum tool argument bytes.
    pub max_request_bytes: usize,
    /// Maximum complete JSON result bytes, including provenance.
    pub max_response_bytes: usize,
    /// Whole admission/call deadline in milliseconds.
    pub timeout_ms: u64,
    /// Total attempted calls for this connection, including denied calls.
    pub max_calls: usize,
    /// Path expansion depth.
    pub max_hops: usize,
    /// Path node visit budget.
    pub max_visits: usize,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_nodes: 10_000,
            max_edges: 20_000,
            max_graph_bytes: 8 * 1024 * 1024,
            max_evidence: 200,
            max_page_size: 100,
            max_request_bytes: 16 * 1024,
            max_response_bytes: 256 * 1024,
            timeout_ms: 10_000,
            max_calls: 500,
            max_hops: 8,
            max_visits: 10_000,
        }
    }
}

impl Budgets {
    fn valid(&self) -> bool {
        [
            (self.max_nodes, 10_000),
            (self.max_edges, 20_000),
            (self.max_graph_bytes, 32 * 1024 * 1024),
            (self.max_evidence, 1_000),
            (self.max_page_size, 200),
            (self.max_request_bytes, 64 * 1024),
            (self.max_response_bytes, 1024 * 1024),
            (self.max_calls, 10_000),
            (self.max_hops, 8),
            (self.max_visits, 10_000),
        ]
        .iter()
        .all(|(v, cap)| *v > 0 && v <= cap)
            && (1..=30_000).contains(&self.timeout_ms)
            && self.max_response_bytes >= 1024
    }
}

/// Immutable authorization decision resolved by Paperclip's trusted connector.
/// A JSON representation is not a credential and must not come from tool args.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadView {
    /// Exact contract version, currently 1.
    pub version: u32,
    /// Unique admission id; never reused across connections.
    pub id: String,
    /// Paperclip policy revision used for admission.
    pub policy_revision: String,
    /// Authenticated admitted identity.
    pub identity: RunIdentity,
    /// Issue time in Unix seconds.
    pub issued_at: u64,
    /// Exclusive expiry in Unix seconds.
    pub expires_at: u64,
    /// Single authorized repository.
    pub repo: String,
    /// Exact published build; no active-pointer fallback.
    pub build_id: String,
    /// Exact authorized snapshot set.
    pub snapshots: BTreeSet<String>,
    /// Allowed repository-relative file/subtree roots; `.` explicitly means all.
    pub source_paths: Vec<String>,
    /// Denied file/subtree roots; exclusions win.
    pub excluded_paths: Vec<String>,
    /// Granted operations; empty is invalid.
    pub operations: BTreeSet<Operation>,
    /// Bounded work and output.
    pub budgets: Budgets,
}

impl ReadView {
    /// Validate an admission against the connector's authenticated identity and
    /// clock. This checks a decision; it does not resolve or grant permissions.
    pub fn validate_at(&self, identity: &RunIdentity, now: u64) -> Result<(), ReadError> {
        if identity != &self.identity {
            return Err(ReadError::Identity);
        }
        let i = &self.identity;
        if self.version != 1
            || !self.budgets.valid()
            || [
                &self.id,
                &self.policy_revision,
                &self.repo,
                &self.build_id,
                &i.company,
                &i.project,
                &i.agent,
                &i.task,
                &i.run,
                &i.host,
                &i.server,
            ]
            .into_iter()
            .any(|s| !valid_label(s))
            || self.snapshots.is_empty()
            || self.snapshots.len() > 100
            || self.snapshots.iter().any(|s| !valid_label(s))
            || self.source_paths.is_empty()
            || self.source_paths.len() > 100
            || self.excluded_paths.len() > 100
            || self.operations.is_empty()
            || self
                .source_paths
                .iter()
                .chain(&self.excluded_paths)
                .any(|p| p != "." && !valid_path(p))
            || self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > 86_400
        {
            return Err(ReadError::InvalidView);
        }
        if now < self.issued_at || now >= self.expires_at {
            return Err(ReadError::Expired);
        }
        super::bounded_json(self, 128 * 1024).map_err(|_| ReadError::InvalidView)?;
        Ok(())
    }

    pub(super) fn allows_file(&self, file: &str) -> bool {
        let under = |root: &String| {
            root == "."
                || file == root
                || file
                    .strip_prefix(root.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        valid_path(file)
            && self.source_paths.iter().any(under)
            && !self.excluded_paths.iter().any(under)
    }
}

fn valid_label(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}

pub(super) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}
