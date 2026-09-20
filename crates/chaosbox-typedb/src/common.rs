//! Shared driver plumbing for the `TypeDB` backend: connection config,
//! bounded timeouts, error classification, row collection, and the
//! deterministic key helpers that replace Gel's exclusive constraints.

use std::collections::BTreeMap;
use std::time::Duration;

use chaosbox_core::deterministic_id;
use chaosbox_gel::GelError;
use futures::TryStreamExt;
use typedb_driver::{
    TransactionOptions, TransactionType, TypeDBDriver,
    answer::{ConceptRow, QueryAnswer},
    concept::{Concept, Value},
};

/// Connection and database selection for one backend handle.
#[derive(Clone, Debug)]
pub struct TypeDbConfig {
    /// Server address, e.g. `127.0.0.1:1729`.
    pub address: String,
    /// Application username (never the bootstrap admin in production).
    pub username: String,
    /// Application password (delivered via credential file upstream).
    pub password: String,
    /// `TypeDB` database name holding the Chaosbox schema and rows.
    pub database: String,
}

/// Bounded transaction lifetimes: staging writes are small, the pointer
/// swing must not hang a publisher forever.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Reads are bounded so a wedged server fails a consumer request instead of
/// hanging it; callers apply their own tighter deadlines on top.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Transient (connection-level) flush retries; conflicts are never retried
/// blindly because a retry could mask a lost publication race.
pub(crate) const FLUSH_RETRIES: usize = 3;

/// True for the `@unique` violation a duplicate `@key` insert raises.
/// Proven shape: code `CNT9`.
pub(crate) fn is_unique_violation(e: &typedb_driver::Error) -> bool {
    e.code() == "CNT9"
}

/// True for a commit-time isolation conflict between concurrent writers.
/// Proven shape: code `STC2`.
pub(crate) fn is_conflict(e: &typedb_driver::Error) -> bool {
    e.code() == "STC2"
}

/// Classify a driver failure for the [`Store`](chaosbox_gel::Store) and
/// [`GelQueries`](chaosbox_gel::GelQueries) surfaces. Takes ownership for
/// direct use as `map_err(driver_error)` across the backend.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn driver_error(e: typedb_driver::Error) -> GelError {
    match &e {
        typedb_driver::Error::Connection(_) => GelError::Client(e.to_string()),
        _ => GelError::Query(format!("[{}] {e}", e.code())),
    }
}

/// Current time as integer epoch millis for `created`/`updated` attributes.
pub(crate) fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

/// Deterministic key for a file version: replaces Gel's exclusive
/// `(snapshot, path)` constraint with a content-derived `@key`.
pub(crate) fn file_version_id(snapshot: &str, path: &str) -> String {
    deterministic_id("fv", &[snapshot, path])
}

/// Deterministic key for a span row.
pub(crate) fn span_id_of(
    file: &str,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    byte_start: u32,
    byte_end: u32,
) -> String {
    deterministic_id(
        "sp",
        &[
            file,
            &start_line.to_string(),
            &start_col.to_string(),
            &end_line.to_string(),
            &end_col.to_string(),
            &byte_start.to_string(),
            &byte_end.to_string(),
        ],
    )
}

/// Deterministic key for a decision: `(candidate, question)` replaces Gel's
/// exclusive constraint of the same shape.
pub(crate) fn decision_key(candidate_id: &str, question_id: &str) -> String {
    deterministic_id("decq", &[candidate_id, question_id])
}

/// Deterministic key for a node-membership relation row.
pub(crate) fn membership_id(build_id: &str, entity_id: &str) -> String {
    deterministic_id("nm", &[build_id, entity_id])
}

/// Deterministic key for an edge-membership relation row.
pub(crate) fn edge_membership_id(build_id: &str, rel_id: &str) -> String {
    deterministic_id("em", &[build_id, rel_id])
}

/// Deterministic key for a claim-evidence link row.
pub(crate) fn link_id(prefix: &str, claim_id: &str, evidence_id: &str) -> String {
    deterministic_id(prefix, &[claim_id, evidence_id])
}

/// Case folding for the `name-fold` search columns (mirrors the Gel `ilike`
/// semantics the reader preserves; ASCII-tested, Unicode `to_lowercase`).
pub(crate) fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// Drain a write answer: rows and documents are collected and dropped so
/// the commit sees a fully-consumed pipeline. The driver error is boxed
/// across the await boundary (`result_large_err`); callers classify the
/// unboxed error before mapping it.
pub(crate) async fn drain(answer: QueryAnswer) -> Result<(), Box<typedb_driver::Error>> {
    match answer {
        QueryAnswer::Ok(_) => Ok(()),
        QueryAnswer::ConceptRowStream(_, stream) => {
            let rows: Vec<ConceptRow> = stream.try_collect().await.map_err(Box::new)?;
            let _ = rows.len();
            Ok(())
        }
        QueryAnswer::ConceptDocumentStream(_, stream) => {
            let docs: Vec<_> = stream.try_collect().await.map_err(Box::new)?;
            let _ = docs.len();
            Ok(())
        }
    }
}

/// Collect a read query into owned column values. Missing optional columns
/// (bound through `try {}`) surface as absent keys.
pub(crate) async fn read_rows(
    driver: &TypeDBDriver,
    db: &str,
    query: &str,
    columns: &[&str],
) -> Result<Vec<BTreeMap<String, Value>>, GelError> {
    let tx = driver
        .transaction_with_options(
            db,
            TransactionType::Read,
            TransactionOptions::new().transaction_timeout(READ_TIMEOUT),
        )
        .await
        .map_err(driver_error)?;
    let answer = tx.query(query).await.map_err(driver_error)?;
    let rows: Vec<ConceptRow> = match answer {
        QueryAnswer::ConceptRowStream(_, stream) => {
            stream.try_collect().await.map_err(driver_error)?
        }
        other => {
            return Err(GelError::Query(format!("expected rows, got {other:?}")));
        }
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut map = BTreeMap::new();
        for col in columns {
            match row.get(col).map_err(driver_error)? {
                None => {}
                Some(Concept::Attribute(attr)) => {
                    map.insert((*col).to_owned(), attr.value.clone());
                }
                Some(other) => {
                    return Err(GelError::Query(format!(
                        "column {col} is not an attribute: {other:?}"
                    )));
                }
            }
        }
        out.push(map);
    }
    Ok(out)
}

/// Required string column.
pub(crate) fn col_string(row: &BTreeMap<String, Value>, col: &str) -> Result<String, GelError> {
    row.get(col)
        .and_then(Value::get_string)
        .map(str::to_owned)
        .ok_or_else(|| GelError::Query(format!("missing string column {col}")))
}

/// Required integer column.
pub(crate) fn col_int(row: &BTreeMap<String, Value>, col: &str) -> Result<i64, GelError> {
    row.get(col)
        .and_then(Value::get_integer)
        .ok_or_else(|| GelError::Query(format!("missing integer column {col}")))
}

/// Required boolean column.
pub(crate) fn col_bool(row: &BTreeMap<String, Value>, col: &str) -> Result<bool, GelError> {
    row.get(col)
        .and_then(Value::get_boolean)
        .ok_or_else(|| GelError::Query(format!("missing boolean column {col}")))
}

/// Optional double column (absent when the `try {}` branch did not bind).
pub(crate) fn col_double_opt(row: &BTreeMap<String, Value>, col: &str) -> Option<f64> {
    row.get(col).and_then(Value::get_double)
}
