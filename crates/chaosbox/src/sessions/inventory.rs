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

use crate::sessions::campaign::{is_digest, is_session_id, CampaignError};

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
    /// Returns [`CampaignError::InvalidProgress`] when a field is absent or
    /// has the wrong shape.
    ///
    /// `total`, `deferred` and `errors` have to be present. They are the
    /// counters that say whether the run that pinned the inventory left work
    /// behind, and defaulting them would invent agreement: `total` defaulted
    /// to the number of `verified` entries matches by construction, and
    /// `deferred`/`errors` defaulted to zero report work as never having been
    /// reported. An inventory that cannot be read leaves `reconciled` false,
    /// and a pass over it can never succeed — not under `--allow-partial`
    /// either, because `clean()` includes reconciliation.
    ///
    /// `verified` has to be an array of objects each naming one distinct
    /// session. `complete` may be absent (which reads as "not finished", the
    /// conservative answer) but must be a boolean when present. The digests
    /// may be absent but must be 64 lowercase hex digits when present.
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

        Ok(Self {
            total: count(progress, "total")?,
            expected,
            deferred: size(progress, "deferred")?,
            errors: size(progress, "errors")?,
            complete: completion(progress)?,
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

/// A required non-negative integer counter.
///
/// It is required rather than defaulted. `total` defaulted to the number of
/// `verified` entries matches by construction, so an inventory that reported
/// no total at all would reconcile perfectly; `deferred` and `errors`
/// defaulted to zero would assert that nothing was left behind. All three are
/// claims, and a claim nobody wrote down is not a claim this pass may make
/// on the driver's behalf.
fn count(progress: &Value, field: &str) -> Result<usize, CampaignError> {
    let value = progress.get(field).ok_or_else(|| required(field))?;
    non_negative(value, field)
}

/// A required counter the driver may write either as a bare number or as the
/// array of sessions behind it.
///
/// The elements are counted, not interpreted: a non-empty array already
/// fails reconciliation, so what the entries look like never decides whether
/// the campaign agrees with itself.
fn size(progress: &Value, field: &str) -> Result<usize, CampaignError> {
    let value = progress.get(field).ok_or_else(|| required(field))?;
    if let Some(entries) = value.as_array() {
        return Ok(entries.len());
    }
    non_negative(value, field)
}

/// A counter shaped the way the driver should have written it.
fn non_negative(value: &Value, field: &str) -> Result<usize, CampaignError> {
    let integer = value
        .as_i64()
        .ok_or_else(|| invalid(field, "expected a non-negative integer"))?;
    if integer < 0 {
        return Err(invalid(field, "expected a non-negative integer"));
    }
    Ok(usize::try_from(integer).unwrap_or(usize::MAX))
}

/// A field the inventory parser needs and `progress.json` did not carry.
fn required(field: &str) -> CampaignError {
    invalid(field, "required field is missing")
}

/// Whether the driver finished its run.
///
/// Absent reads as `false`, which is the conservative answer and is reported
/// as `progress_complete: false` rather than silently granted. Present but
/// mistyped is an error instead of a fallback, so `"true"` cannot decay into
/// `false` and hide behind `--allow-partial`.
fn completion(progress: &Value) -> Result<bool, CampaignError> {
    let Some(value) = progress.get("complete") else {
        return Ok(false);
    };
    value
        .as_bool()
        .ok_or_else(|| invalid("complete", "expected a boolean"))
}

/// An optional digest field: absence is reported as absence, but a digest
/// that is present has to be one.
///
/// These are carried into the report as the campaign's own assertions. The
/// pass echoes what `progress.json` pinned; it does not re-derive them from
/// the artifacts they name, so matching `driverDigest` against the frozen
/// `assemble-canonical-history.mjs` stays G1's job rather than something a
/// clean report silently implies it performed.
fn digest(progress: &Value, field: &str) -> Result<Option<String>, CampaignError> {
    let Some(value) = progress.get(field) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| invalid(field, "expected a hex digest string"))?;
    if !is_digest(text) {
        return Err(invalid(field, "expected 64 lowercase hex digits"));
    }
    Ok(Some(text.to_string()))
}
