//! Connection config, bounded sessions, error classification and the
//! row-encoding helpers shared by the store and the reader.

use std::net::IpAddr;

use chaosbox_store::{EntityRow, StoreError};
use tokio_postgres::{config::Host, error::SqlState, types::ToSql, Client, NoTls, Row, Transaction};

/// Connection selection for one backend handle: a libpq key=value or URL
/// connection string. Sessions are unencrypted, so only unix sockets and
/// loopback addresses are accepted.
#[derive(Clone, PartialEq, Eq)]
pub struct PgConfig {
    conninfo: String,
}

impl std::fmt::Debug for PgConfig {
    // The connection string may carry a password; never print it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PgConfig { .. }")
    }
}

impl PgConfig {
    /// Validate a connection string: every host must be a unix socket or a
    /// loopback address, because sessions use no TLS.
    pub fn new(conninfo: impl Into<String>) -> Result<Self, String> {
        let conninfo = conninfo.into();
        let parsed: tokio_postgres::Config = conninfo
            .parse()
            .map_err(|e| format!("invalid database connection string: {e}"))?;
        let loopback_name = |name: &str| {
            name == "localhost" || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        };
        let local_hosts = parsed.get_hosts().iter().all(|host| match host {
            Host::Tcp(name) => loopback_name(name),
            #[cfg(unix)]
            Host::Unix(_) => true,
        });
        if parsed.get_hosts().is_empty()
            || !local_hosts
            || !parsed.get_hostaddrs().iter().all(IpAddr::is_loopback)
        {
            return Err(
                "database host must be a unix socket directory or a loopback address".into(),
            );
        }
        Ok(Self { conninfo })
    }

    /// Operator configuration from `CHAOSBOX_DATABASE_URL`; there is no default.
    pub fn from_env() -> Result<Self, String> {
        Self::new(
            std::env::var("CHAOSBOX_DATABASE_URL").map_err(|_| "CHAOSBOX_DATABASE_URL unset")?,
        )
    }
}

/// Open one bounded session. A wedged statement fails its caller instead of
/// hanging it, and an abandoned transaction cannot pin locks or snapshots.
pub(crate) async fn connect(config: &PgConfig) -> Result<Client, StoreError> {
    let mut parsed: tokio_postgres::Config = config
        .conninfo
        .parse()
        .map_err(|e| StoreError::Connection(format!("invalid connection string: {e}")))?;
    if parsed.get_application_name().is_none() {
        parsed.application_name("chaosbox");
    }
    let (client, connection) = parsed.connect(NoTls).await.map_err(pg_error)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(
            "SET statement_timeout = '60s'; SET idle_in_transaction_session_timeout = '60s';",
        )
        .await
        .map_err(pg_error)?;
    Ok(client)
}

/// Classify a driver failure. A missing database reads as "not found" (the
/// pending `db check` state); a missing table means migrations never ran.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn pg_error(e: tokio_postgres::Error) -> StoreError {
    match e.code() {
        Some(code) if *code == SqlState::INVALID_CATALOG_NAME => {
            StoreError::Connection(format!("database not found: {e}"))
        }
        Some(code) if *code == SqlState::UNDEFINED_TABLE => {
            StoreError::NotFound(format!("chaosbox schema missing; run db migrate: {e}"))
        }
        Some(code) => StoreError::Query(format!("[{}] {e}", code.code())),
        None => StoreError::Connection(e.to_string()),
    }
}

/// True when a commit outcome is unknown: the session failed, not the server.
pub(crate) fn is_uncertain(e: &tokio_postgres::Error) -> bool {
    e.code().is_none()
}

/// `text` columns reject NUL. Replace it in display/search columns; bodies
/// are JSON, which escapes NUL.
pub(crate) fn clean(s: &str) -> String {
    s.replace('\0', "\u{FFFD}")
}

/// Case folding for the search columns (Rust `to_lowercase`, never the
/// database locale's `lower()`).
pub(crate) fn fold(s: &str) -> String {
    clean(&s.to_lowercase())
}

/// Deterministic key for a decision: one row per `(candidate, question)`.
pub(crate) fn decision_key(candidate_id: &str, question_id: &str) -> String {
    chaosbox_core::deterministic_id("decq", &[candidate_id, question_id])
}

/// Content address of one embedding input.
#[must_use]
pub fn text_sha256(body: &str) -> String {
    chaosbox_core::sha256_hex(&["chaosbox-text-v1", body])
}

/// Encode a vector as the pgvector text form `[x,y,...]`.
pub(crate) fn vector_literal(vector: &[f32]) -> Result<String, StoreError> {
    if vector.iter().any(|x| !x.is_finite()) {
        return Err(StoreError::Invariant(
            "non-finite embedding component".into(),
        ));
    }
    let parts: Vec<String> = vector.iter().map(f32::to_string).collect();
    Ok(format!("[{}]", parts.join(",")))
}

/// Bulk insert rows (each `N` text columns, NULL allowed) through one
/// `UNNEST` statement per chunk. `sql` names the target and the select list
/// over `UNNEST($1::text[], …) AS t(c1, …)`; rows are written in the given
/// order, so callers pass key order and concurrent writers lock keys
/// consistently.
// ponytail: 10k rows per statement bounds client and server memory; tune only
// if publication profiles show statement overhead.
pub(crate) async fn insert_rows<const N: usize>(
    tx: &Transaction<'_>,
    sql: &str,
    rows: Vec<[Option<String>; N]>,
) -> Result<(), StoreError> {
    const CHUNK: usize = 10_000;
    let mut rows = rows.into_iter().peekable();
    while rows.peek().is_some() {
        let mut columns: Vec<Vec<Option<String>>> = vec![Vec::new(); N];
        for row in rows.by_ref().take(CHUNK) {
            for (column, value) in columns.iter_mut().zip(row) {
                column.push(value);
            }
        }
        let params: Vec<&(dyn ToSql + Sync)> = columns
            .iter()
            .map(|column| column as &(dyn ToSql + Sync))
            .collect();
        tx.execute(sql, &params).await.map_err(pg_error)?;
    }
    Ok(())
}

/// Entity columns every member query selects, in [`entity_from_row`] order.
pub(crate) const ENTITY_COLUMNS: &str =
    "e.entity_id, e.kind, e.repo, e.snapshot_id, e.file, e.name, e.qualified_name, e.span, e.compiler";

/// Decode one [`ENTITY_COLUMNS`] row.
pub(crate) fn entity_from_row(row: &Row) -> Result<EntityRow, StoreError> {
    let span: String = row.get(7);
    let compiler: Option<String> = row.get(8);
    Ok(EntityRow {
        entity_id: row.get(0),
        kind: row.get(1),
        repo: row.get(2),
        snapshot: row.get(3),
        file: row.get(4),
        name: row.get(5),
        qualified_name: row.get(6),
        span: Some(
            serde_json::from_str(&span)
                .map_err(|e| StoreError::Query(format!("invalid source span: {e}")))?,
        ),
        compiler: compiler
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|e| StoreError::Query(format!("invalid compiler identity: {e}")))?,
    })
}

/// Encode any serializable payload as an exact JSON body.
pub(crate) fn json<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Invariant(e.to_string()))
}

/// Non-blocking transaction-scoped advisory lock on one logical key. False
/// means a concurrent writer holds it; callers report a lost race.
pub(crate) async fn try_lock(tx: &Transaction<'_>, key: &str) -> Result<bool, StoreError> {
    Ok(tx
        .query_one(
            "SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))",
            &[&key],
        )
        .await
        .map_err(pg_error)?
        .get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_same_host_transports_are_accepted() {
        for ok in [
            "host=/run/postgresql dbname=chaosbox",
            "host=localhost dbname=chaosbox",
            "host=127.0.0.1 port=5432 user=can dbname=chaosbox",
            "host=::1 dbname=chaosbox",
            "postgresql://can@localhost/chaosbox",
        ] {
            assert!(PgConfig::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "host=db.example.com dbname=chaosbox",
            "host=10.0.0.5 dbname=chaosbox",
            "host=/run/postgresql,db.example.com dbname=chaosbox",
            "host=localhost hostaddr=10.0.0.5 dbname=chaosbox",
            "dbname=chaosbox",
            "not a conninfo '",
        ] {
            assert!(PgConfig::new(bad).is_err(), "{bad}");
        }
        assert_eq!(
            format!("{:?}", PgConfig::new("host=/tmp password=secret").unwrap()),
            "PgConfig { .. }"
        );
    }

    #[test]
    fn vectors_encode_exactly_and_reject_non_finite() {
        assert_eq!(vector_literal(&[0.5, -1.0, 0.0]).unwrap(), "[0.5,-1,0]");
        assert!(vector_literal(&[f32::NAN]).is_err());
    }
}
