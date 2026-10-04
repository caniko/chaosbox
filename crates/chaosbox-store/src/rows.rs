//! Typed `query_json` row shapes and the projections that build them
//! from domain types.

use chaosbox_core::{Entity, Relation, SourceSpan, coverage::BuildCoverage};
use serde::{Deserialize, Serialize};

/// Typed row for entity lookup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityRow {
    /// Entity id.
    pub entity_id: String,
    /// Entity kind name.
    pub kind: String,
    /// Owning repository name.
    pub repo: String,
    /// Snapshot this identity belongs to.
    pub snapshot: String,
    /// Repository-relative file path.
    pub file: String,
    /// Short display name.
    pub name: String,
    /// Qualified name.
    pub qualified_name: String,
    /// Exact occurrence location; absent in older projections.
    #[serde(default)]
    pub span: Option<SourceSpan>,
    /// Compiler anchor/context when this is a compiler occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<chaosbox_core::compiler::SymbolIdentity>,
}

/// Published build header row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildRow {
    /// Build id.
    pub build_id: String,
    /// Monotonic generation.
    pub generation: i64,
    /// `staging` or `active`. Here `active` means published; historical builds
    /// retain that status after the repository's active pointer advances.
    pub status: String,
    /// Snapshot ids pinned by this build (freshness fingerprint), sorted
    /// for deterministic reporting. Readers whose projection predates the
    /// field default to an empty list rather than failing the decode.
    #[serde(default)]
    pub snapshots: Vec<String>,
    /// Processing coverage of this immutable build; unknown for legacy builds.
    #[serde(default)]
    pub coverage: Option<BuildCoverage>,
    /// Publication sealed its complete evidence closure. Legacy headers remain
    /// available for operator diffs, but cannot authorize run-bound readers.
    #[serde(default)]
    pub evidence_sealed: bool,
}

/// Relationship row with endpoint ids.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelRow {
    /// Relationship id.
    pub rel_id: String,
    /// Relation type name.
    pub rel_type: String,
    /// Source endpoint wrapper.
    pub from_entity: EndpointRef,
    /// Target endpoint wrapper.
    pub to_entity: EndpointRef,
}

/// Endpoint id wrapper around one entity id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointRef {
    /// Entity id.
    pub entity_id: String,
}

/// Evidence row for claim support/contradiction display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceRow {
    /// Evidence id.
    pub evidence_id: String,
    /// Evidence class name.
    pub class: String,
    /// True when supporting the relationship.
    pub supports: bool,
    /// Source-copied or template text.
    pub text: String,
    /// Source identity and exact range, when retained by the backend.
    #[serde(default)]
    pub citation: Option<SourceCitation>,
    /// Versioned parser provenance for direct facts; absent for model evidence.
    #[serde(default)]
    pub producer: Option<String>,
}

/// Immutable source citation. The hash uses Chaosbox's snapshot text hashing
/// convention (`sha256_hex`), including its trailing separator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCitation {
    /// Snapshot containing these source bytes.
    pub snapshot: String,
    /// Repository-relative file path.
    pub file: String,
    /// Content identity stored during capture, never synthesized by a reader.
    pub sha256: String,
    /// Exact range; legacy/model evidence may not have one.
    pub span: Option<SourceSpan>,
}

/// Project one entity into its row form (canonical storage names).
pub(crate) fn entity_row(e: &Entity) -> EntityRow {
    EntityRow {
        entity_id: e.id.clone(),
        kind: chaosbox_core::entity_kind_name(&e.kind),
        repo: e.repo.clone(),
        snapshot: e.snapshot.clone(),
        file: e.file.clone(),
        name: e.name.clone(),
        qualified_name: e.qualified_name.clone(),
        span: Some(e.span.clone()),
        compiler: e.compiler.clone(),
    }
}

/// Project one relationship into its row form (canonical storage names).
pub(crate) fn rel_row(r: &Relation) -> RelRow {
    RelRow {
        rel_id: r.id.clone(),
        rel_type: chaosbox_core::relation_type_name(&r.rel_type),
        from_entity: EndpointRef {
            entity_id: r.from.clone(),
        },
        to_entity: EndpointRef {
            entity_id: r.to.clone(),
        },
    }
}
