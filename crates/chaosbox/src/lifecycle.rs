//! Versioned lifecycle report envelope behind `db check` and `db migrate`.

use super::{Serialize, Deserialize};

// ---- Lifecycle contract (db check / db migrate) ----

/// Versioned JSON envelope. Diagnostics go to stderr; stdout is this JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LifecycleReport {
    /// Contract version (currently 2).
    pub contract_version: u32,
    /// Storage backend label (`typedb`).
    pub backend: String,
    /// Operation name (`db check`, `db migrate`).
    pub operation: String,
    /// `ready`, `pending`, or `error`.
    pub status: String,
    /// Schema compatibility marker.
    pub schema_version: u32,
    /// Pinned backend version the schema targets.
    pub pinned: String,
    /// Sanitized machine-readable detail (never secret values).
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl LifecycleReport {
    /// Shared builder (contract v2): the envelope carries the pinned
    /// `TypeDB` version the schema targets.
    fn report(operation: &str, status: &str, detail: serde_json::Value) -> Self {
        Self {
            contract_version: 2,
            backend: "typedb".into(),
            operation: operation.into(),
            status: status.into(),
            schema_version: chaosbox_typedb::SCHEMA_VERSION,
            pinned: chaosbox_typedb::TYPEDB_PINNED.into(),
            detail,
        }
    }

    /// A ready report: exit 0 after the caller prints it.
    #[must_use]
    pub fn check_ready(detail: serde_json::Value) -> Self {
        Self::report("db check", "ready", detail)
    }

    /// A non-ready report: the caller prints it and exits nonzero.
    #[must_use]
    pub fn pending(operation: &str, reason: &str) -> Self {
        Self::report(operation, "pending", serde_json::json!({"reason": reason}))
    }

    /// An operational-error report: sanitized diagnostics, stdout stays parseable.
    #[must_use]
    pub fn error(operation: &str, reason: &str) -> Self {
        Self::report(operation, "error", serde_json::json!({"reason": reason}))
    }
}
