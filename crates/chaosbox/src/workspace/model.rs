use std::{collections::BTreeMap, path::PathBuf};
use chaosbox_core::GraphBuild;
use chaosbox_extract::Snapshot;
use chaosbox_store::SourceCitation;
use serde::{Deserialize, Serialize};

/// Reviewed input recipe. Bridges are explicit interpretation, never compiler facts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spec {
    /// Exact private visibility label required by every query.
    pub scope: String,
    /// Repository namespace to selected source files.
    pub members: BTreeMap<String, MemberSpec>,
    /// Human-readable endpoint labels to exact, unique source quotations.
    pub endpoints: BTreeMap<String, EndpointSpec>,
    /// Directed impact links: changed provider -> affected consumer.
    pub bridges: Vec<Bridge>,
    /// Source-backed reviewed constraints; not model-admitted memory.
    pub constraints: Vec<Constraint>,
}

/// An explicitly bounded member corpus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberSpec {
    /// Root of the source checkout.
    pub root: PathBuf,
    /// Exact repository-relative source/configuration files to capture.
    pub files: Vec<String>,
}

/// Source selection for one impact endpoint or evidence item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointSpec {
    /// Owning member name.
    pub member: String,
    /// Selected file.
    pub file: String,
    /// Verbatim, nonempty, uniquely occurring quotation (at most 8 KiB).
    pub quote: String,
}

/// A reviewed bridge, tied to both endpoints' exact member builds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bridge {
    /// Provider/change endpoint label.
    pub from: String,
    /// Affected dependent endpoint label.
    pub to: String,
    /// Reviewer's evidence-bounded interpretation.
    pub reason: String,
    /// Endpoint labels supplying source citations for the bridge.
    pub evidence: Vec<String>,
    /// Required on cross-repository bridges; read from captured consumer JSON.
    pub pin: Option<DependencyPin>,
}

/// A dependency revision in a captured lockfile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyPin {
    /// Consumer member, which must own the bridge's `to` endpoint.
    pub member: String,
    /// Captured JSON lockfile.
    pub file: String,
    /// JSON pointer to the expected provider Git revision.
    pub pointer: String,
}

/// Grounded engineering guidance admitted by explicit review for this pilot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Constraint {
    /// Stable human-readable identifier in this artifact.
    pub id: String,
    /// Reviewed statement, returned as historical data rather than instructions.
    pub statement: String,
    /// Applicable impact endpoint labels.
    pub applies_to: Vec<String>,
    /// Endpoint labels citing its grounding sources.
    pub evidence: Vec<String>,
}

/// Immutable selected-file graph and version provenance for one member.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    /// Local source root, used only for freshness checks.
    pub root: PathBuf,
    /// Observed Git HEAD, if this root is a Git checkout.
    pub revision: Option<String>,
    /// Checkout dirty, selected bytes absent/different at HEAD, or Git provenance
    /// unavailable. A clean status alone cannot bind ignored/assume-unchanged files.
    pub dirty: bool,
    /// Pinned sources, including historical source bytes.
    pub snapshot: Snapshot,
    /// Exact zero-model local build, not the active `TypeDB` pointer.
    pub build: GraphBuild,
}

/// Source citation bound to an entity in one immutable member build.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    /// Owning member namespace.
    pub member: String,
    /// Exact member build ID.
    pub build: String,
    /// File entity in that build. Quote span narrows the cited location.
    pub entity: String,
    /// Historical source identity and range.
    pub citation: SourceCitation,
    /// Exact source quotation.
    pub quote: String,
}

/// Immutable local workspace pilot. It is not a general graph publication or
/// an authenticated attestation of the reviewer's interpretation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    /// Full content fingerprint, excluding this field itself.
    pub id: String,
    /// Artifact contract version.
    pub contract: String,
    /// Visibility boundary, matched before any source is read or returned.
    pub scope: String,
    /// Exact member builds and selected file corpora.
    pub members: BTreeMap<String, Member>,
    /// Source-cited endpoints/evidence.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Reviewed, directed dependency bridges.
    pub bridges: Vec<Bridge>,
    /// Explicitly reviewed constraints.
    pub constraints: Vec<Constraint>,
}
