//! Optional, receipt-bound SCIP protobuf ingestion. No indexer is required for
//! normal syntax extraction. References, calls, and implementations are distinct.

mod context;
mod normalize;
mod positions;

pub use context::{AnalysisInputs, AnalysisSettings, Encoding, Receipt, binary_hash};
pub use normalize::{CompilerExtraction, normalize};

/// Invalid or stale compiler input. A failed import publishes nothing.
#[derive(Debug, thiserror::Error)]
#[error("compiler evidence: {0}")]
pub struct CompilerError(pub String);

impl From<crate::ExtractError> for CompilerError {
    fn from(value: crate::ExtractError) -> Self {
        Self(value.to_string())
    }
}

impl From<std::io::Error> for CompilerError {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}

fn invalid(message: impl Into<String>) -> CompilerError {
    CompilerError(message.into())
}

/// Strict portable repository-relative path, without normalization surprises.
pub(crate) fn relative_path(path: &str) -> Result<(), CompilerError> {
    if path.is_empty()
        || path.contains(['\\', ':', '\0'])
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(invalid(format!("invalid relative path {path:?}")));
    }
    Ok(())
}
