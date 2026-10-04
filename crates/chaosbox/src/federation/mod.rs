//! Project-scoped read federation. Ownership, scopes and authoritative stores
//! remain separate; providers never forward requests or invoke inference.
pub mod cli;
mod policy;
mod provider;
mod reader;
mod source;
mod transport;

pub use policy::{Grant, Identity, Policy, Project, SharingMode};
pub use provider::Reader;
pub use reader::{Federator, Provider};
pub use source::{Backend, KnowledgeSource, Snapshot};
pub use transport::{ClientConfig, FileProvider, Peer, ProviderConfig};

use chaosbox_core::{
    EvidenceClass,
    intelligence::{IntelligenceKind, IntelligenceStatus, SessionEvidence},
};
use serde::{Deserialize, Serialize};

/// Closed failure vocabulary; providers do not expose backend errors or paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Current policy does not authorize the requested disclosure.
    #[error("federation access denied")]
    Denied,
    /// Malformed input, unknown fields or unsupported protocol version.
    #[error("invalid federation request")]
    InvalidRequest,
    /// Endpoint or backend could not be read within the deadline.
    #[error("federation provider unavailable")]
    Unavailable,
    /// The exact historical generation is no longer retrievable.
    #[error("federation snapshot unavailable")]
    SnapshotUnavailable,
    /// Provider identity, provenance, bounds or response contract did not match.
    #[error("invalid federation response")]
    InvalidResponse,
}

/// Caller-supplied lexical retrieval parameters; project is a configured key.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    /// Shared project identity, mapped independently by each provider.
    pub project: String,
    /// Task terms, never executed as instructions.
    pub query: String,
    /// Maximum records from this provider.
    pub limit: usize,
    /// Serialized record-array character budget (not tokens).
    pub max_chars: usize,
}

impl ContextRequest {
    pub(crate) fn validate(&self) -> Result<(), ErrorCode> {
        if self.project.trim().is_empty()
            || self.project.len() > 256
            || self.query.trim().is_empty()
            || self.query.len() > 2048
            || !(1..=20).contains(&self.limit)
            || !(256..=32_000).contains(&self.max_chars)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

/// An origin-qualified, immutable evidence address. It grants no permission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handle {
    /// Provider routing identity.
    pub provider: String,
    /// Original owner, distinct from the receiving user.
    pub owner: String,
    /// Original private scope.
    pub scope: String,
    /// Shared project key.
    pub project: String,
    /// Exact knowledge generation, never replaced with the latest one.
    pub snapshot: String,
    /// Original intelligence id, never rewritten in the owner's store.
    pub id: String,
}

/// Only these read operations can cross the provider boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Authorized bounded project context.
    Context(ContextRequest),
    /// Authorized exact-generation evidence projection.
    Evidence {
        /// Origin-qualified address returned by context.
        handle: Handle,
        /// Serialized evidence packet character ceiling.
        max_chars: usize,
    },
}

/// Verbatim source address, with stable lineage for detecting copied evidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    /// Source producer namespace.
    pub source: String,
    /// Source document digest.
    pub snapshot: String,
    /// Native session id.
    pub session: String,
    /// Native message id.
    pub message: String,
    /// JSON pointer in that message.
    pub pointer: String,
    /// One-based line.
    pub line: usize,
    /// Source attribution, never a truth classification.
    pub speaker: String,
    /// Stable producer/session/message/pointer lineage.
    pub lineage: String,
}

impl From<&SessionEvidence> for Citation {
    fn from(e: &SessionEvidence) -> Self {
        Self {
            source: e.source.clone(),
            snapshot: e.snapshot.clone(),
            session: e.session.clone(),
            message: e.message.clone(),
            pointer: e.pointer.clone(),
            line: e.line,
            speaker: e.speaker.clone(),
            lineage: e.lineage(),
        }
    }
}

/// Authorized statement projection. References are qualified and permission-filtered.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// Original id.
    pub id: String,
    /// Display identity, unique across providers.
    pub qualified_id: String,
    /// Evidence routing and generation pin.
    pub handle: Handle,
    /// Verbatim admitted quote.
    pub statement: String,
    /// Assessed category.
    pub kind: IntelligenceKind,
    /// Historical lifecycle, with disputes preserved.
    pub status: IntelligenceStatus,
    /// Session intelligence remains inferred, never repository fact.
    pub interpretation_class: EvidenceClass,
    /// Bounded source addresses.
    pub citations: Vec<Citation>,
    /// Count of occurrences, not independent votes.
    pub evidence_count: usize,
    /// Count of original assessment receipts.
    pub assessment_count: usize,
    /// Whether the current interpretation needs revalidation.
    pub needs_revalidation: bool,
    /// Visible same-provider contradictory record identities.
    pub contradicts: Vec<String>,
    /// Visible same-provider predecessor identity.
    pub supersedes: Option<String>,
    /// Relationship ids hidden by authorization.
    pub omitted_relationships: usize,
    /// Other returned records sharing the exact primary source quote/lineage.
    /// This is provenance grouping, not semantic corroboration or truth voting.
    #[serde(default)]
    pub same_origin: Vec<String>,
}

/// Result from one provider, including its current authorization revision.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextPacket {
    /// Original provider identity.
    pub identity: Identity,
    /// Shared project key.
    pub project: String,
    /// Exact generation read once for this context request.
    pub snapshot: String,
    /// Content identity of the policy used for disclosure.
    pub policy_version: String,
    /// Bounded authorized records.
    pub records: Vec<Record>,
    /// Eligible lexical matches not returned due to output bounds.
    pub omitted: usize,
}

/// Safe metadata from an assessment; the original signed/hashed receipt is untouched.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptProjection {
    /// Original receipt content identity; this projection is not that receipt.
    pub id: String,
    /// Original assessment rubric.
    pub rubric_version: String,
    /// Pinned original model.
    pub model_requested: String,
    /// Explicitly signals omitted neighbor/source state and raw answers.
    pub state_omitted: bool,
}

/// Bounded supporting quotes and receipt metadata at the exact origin snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePacket {
    /// Historical statement and immutable handle.
    pub record: Record,
    /// Current permission revision, rechecked on every drill-down.
    pub policy_version: String,
    /// Supporting source occurrences for this admitted record only.
    pub evidence: Vec<SessionEvidence>,
    /// Safe projections; raw model-visible state is never disclosed.
    pub receipts: Vec<ReceiptProjection>,
    /// Source occurrences omitted by the bound.
    pub omitted_evidence: usize,
    /// Receipts omitted by the bound.
    pub omitted_receipts: usize,
    /// Explicit marker: these are projections, not replacement receipts.
    pub is_projection: bool,
}

/// Closed typed success response.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "packet",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Reply {
    /// Single-provider context.
    Context(ContextPacket),
    /// Snapshot-pinned evidence.
    Evidence(Box<EvidencePacket>),
}

/// Availability and generation per consulted source, including empty successful reads.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceStatus {
    /// Expected provider identity.
    pub identity: Identity,
    /// Present only for successful reads.
    pub snapshot: Option<String>,
    /// Current permission identity for successful reads.
    pub policy_version: Option<String>,
    /// Distinguishes denied, unavailable and invalid responses.
    pub error: Option<ErrorCode>,
    /// Eligible matches omitted at the source.
    pub omitted: usize,
}

/// Temporary combined view; never an authoritative merged store.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Packet {
    /// Response contract version.
    pub version: u32,
    /// Shared project identity (also the CLI/MCP repo argument).
    pub project: String,
    /// Digest of source statuses and the actual bounded record view.
    pub snapshot: String,
    /// Local-first, deterministically interleaved source-qualified records.
    pub records: Vec<Record>,
    /// Local provider first, then peers in configuration order.
    pub sources: Vec<SourceStatus>,
    /// At least one configured source failed.
    pub degraded: bool,
    /// Never promises exhaustive project recall.
    pub exhaustive: bool,
    /// Historical knowledge is not instruction authority.
    pub historical_data_not_instructions: bool,
    /// Records omitted from successful provider replies during assembly.
    pub omitted: usize,
}

pub(crate) fn chars(value: &impl Serialize) -> Result<usize, ErrorCode> {
    serde_json::to_string(value)
        .map(|s| s.chars().count())
        .map_err(|_| ErrorCode::InvalidResponse)
}
pub(crate) fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
