//! Versioned, build-local processing coverage; not a completeness claim.

use serde::{Deserialize, Serialize};

/// Outcome of the certified syntax pass for one captured file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxStatus {
    /// Parsed successfully under the producer's documented declaration subset.
    Parsed,
    /// The parser rejected the file; no facts from it were certified.
    ParseError,
    /// This format only has the legacy heuristic extraction path.
    Heuristic,
}

/// Captured-file coverage pinned by a graph build's snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCoverage {
    /// Repository-relative file path.
    pub file: String,
    /// Parser, grammar and extraction-contract version.
    pub producer: String,
    /// Syntax-pass outcome, independent of semantic resolution.
    pub status: SyntaxStatus,
    /// Number of directly published facts in this file.
    pub facts: usize,
}

/// Processing accounting stored with an immutable build.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildCoverage {
    /// Per-file syntax coverage; applies only to captured files.
    pub files: Vec<FileCoverage>,
    /// Relationships published from certified syntax evidence.
    pub structural_relations: usize,
    /// Relationships published from accepted model decisions.
    pub decision_relations: usize,
    /// Optional compiler analysis; absence means it was not requested.
    #[serde(default)]
    pub compiler: Option<crate::compiler::CompilerCoverage>,
}

impl BuildCoverage {
    /// Bounded consumer projection. Full per-file records stay in storage;
    /// large repositories cannot flood status/MCP output with file details.
    #[must_use]
    pub fn report(&self) -> CoverageReport<'_> {
        let shown = self.files.len().min(100);
        CoverageReport {
            structural_relations: self.structural_relations,
            decision_relations: self.decision_relations,
            file_count: self.files.len(),
            parse_errors: self
                .files
                .iter()
                .filter(|f| f.status == SyntaxStatus::ParseError)
                .count(),
            heuristic_files: self
                .files
                .iter()
                .filter(|f| f.status == SyntaxStatus::Heuristic)
                .count(),
            files: &self.files[..shown],
            omitted_files: self.files.len() - shown,
            compiler: self
                .compiler
                .as_ref()
                .map(crate::compiler::CompilerCoverage::report),
        }
    }
}

/// Bounded status/export representation of stored processing coverage.
#[derive(Serialize)]
pub struct CoverageReport<'a> {
    /// Directly published syntax relationships.
    pub structural_relations: usize,
    /// Decision-backed relationships.
    pub decision_relations: usize,
    /// Total captured files, including omitted detail rows.
    pub file_count: usize,
    /// Files whose certified syntax pass failed.
    pub parse_errors: usize,
    /// Files with only legacy heuristic extraction.
    pub heuristic_files: usize,
    /// First 100 per-file coverage records, in capture order.
    pub files: &'a [FileCoverage],
    /// Detail rows not included in this bounded response.
    pub omitted_files: usize,
    /// Bounded compiler coverage with separate capability accounting.
    pub compiler: Option<crate::compiler::CompilerReport<'a>>,
}
