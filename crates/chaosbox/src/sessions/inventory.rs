//! The pinned inventory a verification pass is measured against.
//!
//! Completeness needs an expectation that does not come from the thing being
//! checked. An empty journal has no receipts to disagree with, so a pass that
//! compares the destination only to the receipts it discovered can report
//! success over nothing at all. The driver writes `progress.json` before and
//! during its run and lists every session it intended to migrate, which makes
//! it the yardstick: receipts and destination rows are then reconciled
//! *against* it rather than against each other.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::sessions::campaign::{is_session_id, CampaignError};

/// The session inventory a campaign pins, plus the counters that say whether
/// the run that pinned it finished.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    /// What the driver expected to migrate, from `progress.total`.
    pub total: usize,
    /// Session ids the driver recorded as migrated, from `progress.verified`.
    pub expected: BTreeSet<String>,
    /// Sessions the driver deliberately left behind, from `progress.deferred`.
    pub deferred: usize,
    /// Sessions the driver could not migrate, from `progress.errors`.
    pub errors: usize,
    /// Whether the driver finished its run, from `progress.complete`.
    pub complete: bool,
    /// Digest pinning the campaign identity, from `progress.identityDigest`.
    pub identity_digest: Option<String>,
    /// Digest pinning the frozen migration script, from `progress.driverDigest`.
    pub driver_digest: Option<String>,
}

impl Inventory {
    /// Read the pinned inventory out of a parsed `progress.json`.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::InvalidProgress`] when a field has the wrong
    /// shape: `verified` has to be an array of objects each naming one
    /// distinct session, and the counters have to be non-negative integers
    /// (or, for `deferred` and `errors`, arrays of the sessions behind them).
    pub fn from_progress(progress: &Value) -> Result<Self, CampaignError> {
        let entries = progress
            .get("verified")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("verified", "expected an array of verified sessions"))?;

        let mut expected = BTreeSet::new();
        for (index, entry) in entries.iter().enumerate() {
            let field = format!("verified[{index}].id");
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid(&field, "expected a session id string"))?;
            if !is_session_id(id) {
                return Err(invalid(&field, format!("`{id}` is not a session id")));
            }
            if !expected.insert(id.to_string()) {
                return Err(invalid(&field, format!("`{id}` is listed twice")));
            }
        }

        let default = i64::try_from(expected.len()).unwrap_or(i64::MAX);
        Ok(Self {
            total: count(progress, "total", default)?,
            expected,
            deferred: size(progress, "deferred")?,
            errors: size(progress, "errors")?,
            complete: progress
                .get("complete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            identity_digest: digest(progress, "identityDigest")?,
            driver_digest: digest(progress, "driverDigest")?,
        })
    }
}

/// A `progress.json` field that does not describe an inventory.
fn invalid(field: &str, problem: impl std::fmt::Display) -> CampaignError {
    CampaignError::InvalidProgress {
        field: field.to_string(),
        problem: problem.to_string(),
    }
}

/// A non-negative integer counter, defaulting when the field is absent so a
/// driver that never wrote one still yields a usable expectation.
fn count(progress: &Value, field: &str, default: i64) -> Result<usize, CampaignError> {
    let Some(value) = progress.get(field) else {
        return Ok(usize::try_from(default).unwrap_or(0));
    };
    let integer = value
        .as_i64()
        .ok_or_else(|| invalid(field, "expected a non-negative integer"))?;
    if integer < 0 {
        return Err(invalid(field, "expected a non-negative integer"));
    }
    Ok(usize::try_from(integer).unwrap_or(usize::MAX))
}

/// A counter that the driver may write either as a bare number or as the
/// array of sessions behind it.
fn size(progress: &Value, field: &str) -> Result<usize, CampaignError> {
    let Some(value) = progress.get(field) else {
        return Ok(0);
    };
    if let Some(entries) = value.as_array() {
        return Ok(entries.len());
    }
    count(progress, field, 0)
}

/// An optional digest field: absent is allowed, a non-string is not.
fn digest(progress: &Value, field: &str) -> Result<Option<String>, CampaignError> {
    let Some(value) = progress.get(field) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| invalid(field, "expected a hex digest string"))?;
    Ok(Some(text.to_string()))
}
