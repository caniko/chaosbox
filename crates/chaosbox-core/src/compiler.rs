//! Compiler-specific identity and coverage, separate from syntax and inference.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// An additive logical anchor on an immutable compiler occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolIdentity {
    /// Repository + full SCIP symbol; locals also include document and context.
    pub anchor: String,
    /// Exact analysis input/producer/index fingerprint.
    pub context: String,
    /// Original package-qualified SCIP symbol (never a display-name match).
    pub symbol: String,
    /// Local symbols are not comparable between documents or analyses.
    pub local: bool,
    /// Original SCIP role bitset; this does not imply call semantics.
    pub roles: i32,
}

/// Reproducibility record, not a claim of hermetic compiler execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisContext {
    /// Content fingerprint of the capture receipt.
    pub id: String,
    /// Adapter, official schema binding, and producer versions.
    pub producer: String,
    /// Raw binary index SHA-256 (without the snapshot text separator).
    pub index_sha256: String,
    /// Source/configuration input hashes, using the snapshot text convention.
    pub inputs: BTreeMap<String, String>,
    /// Exact command template; `{index}` denotes the newly created output.
    pub command: Vec<String>,
    /// Operator-declared effective toolchain, configuration, target, features,
    /// and any additional external/environmental inputs.
    pub declared: BTreeMap<String, String>,
    /// Explicit unspecified-position-encoding override, if supplied.
    pub encoding_override: Option<String>,
}

/// One document's processing status and occurrence counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerFileCoverage {
    /// Repository-relative path.
    pub file: String,
    /// `indexed`, `omitted_by_indexer`, or `outside_source_scope`.
    pub status: String,
    /// Effective position encoding; unknown when the document was omitted.
    pub encoding: Option<String>,
    /// Number of occurrences supplied by the producer.
    pub occurrences: usize,
}

/// Explicit compiler capabilities and omissions for an immutable build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerCoverage {
    /// Complete capture provenance.
    pub context: AnalysisContext,
    /// Per-document records, including source files absent from the index.
    pub files: Vec<CompilerFileCoverage>,
    /// Published file-to-definition observations.
    pub definitions: usize,
    /// Published occurrence-to-definition references.
    pub references: usize,
    /// References lacking an in-scope definition; may be external or unresolved.
    pub unresolved_references: usize,
    /// References with multiple definition occurrences; none is selected.
    pub ambiguous_references: usize,
    /// Occurrences without a symbol (e.g. diagnostics-only records).
    pub symbol_less: usize,
    /// Published explicit implementation relationships.
    pub implementations: usize,
    /// Implementation records lacking unique in-scope endpoints.
    pub unresolved_implementations: usize,
    /// Other symbol relationships retained only in the pinned index.
    pub unsupported_relationships: usize,
    /// Diagnostic records supplied by the producer, not compiler success.
    pub diagnostics: usize,
    /// Always `unsupported`: occurrence roles do not distinguish calls.
    pub calls: String,
    /// Always `unknown`: neither producer exit status nor no diagnostics proves it.
    pub typecheck: String,
    /// Always `unknown`: e.g. rust-analyzer may retain inactive references.
    pub configuration_membership: String,
}

impl CompilerCoverage {
    /// Number of directly materialized compiler relationships.
    #[must_use]
    pub fn relations(&self) -> usize {
        self.definitions + self.references + self.implementations
    }

    /// Bounded query representation. Full input inventory stays in storage.
    #[must_use]
    pub fn report(&self) -> CompilerReport<'_> {
        let shown = self.files.len().min(100);
        CompilerReport {
            context: &self.context.id,
            producer: &self.context.producer,
            index_sha256: &self.context.index_sha256,
            files: &self.files[..shown],
            omitted_files: self.files.len() - shown,
            definitions: self.definitions,
            references: self.references,
            unresolved_references: self.unresolved_references,
            ambiguous_references: self.ambiguous_references,
            implementations: self.implementations,
            unresolved_implementations: self.unresolved_implementations,
            unsupported_relationships: self.unsupported_relationships,
            diagnostics: self.diagnostics,
            symbol_less: self.symbol_less,
            calls: &self.calls,
            typecheck: &self.typecheck,
            configuration_membership: &self.configuration_membership,
        }
    }
}

/// Bounded compiler status, including independently reported capabilities.
#[derive(Serialize)]
pub struct CompilerReport<'a> {
    /// Analysis fingerprint.
    pub context: &'a str,
    /// Producer/adapter provenance.
    pub producer: &'a str,
    /// Raw index content hash.
    pub index_sha256: &'a str,
    /// At most 100 documents.
    pub files: &'a [CompilerFileCoverage],
    /// Documents not shown.
    pub omitted_files: usize,
    /// Published definitions.
    pub definitions: usize,
    /// Published references.
    pub references: usize,
    /// References without an in-scope definition.
    pub unresolved_references: usize,
    /// References to multiple definitions.
    pub ambiguous_references: usize,
    /// Published explicit implementations.
    pub implementations: usize,
    /// Implementation records with unresolved endpoints.
    pub unresolved_implementations: usize,
    /// Other symbol relationships omitted from publication.
    pub unsupported_relationships: usize,
    /// Reported diagnostics; absence is not compiler success.
    pub diagnostics: usize,
    /// Occurrences without symbols.
    pub symbol_less: usize,
    /// Call capability.
    pub calls: &'a str,
    /// Typecheck status.
    pub typecheck: &'a str,
    /// Configuration membership status.
    pub configuration_membership: &'a str,
}
