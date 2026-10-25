//! Driver-backed `TypeDB` [`Store`](chaosbox_store::Store) implementation.
//!
//! Write path: every `put_*` validates
//! into an in-memory [`MemoryStore`](chaosbox_store::MemoryStore) staging
//! area with identical semantics, and [`TypeDbStore::publish`] flushes the
//! staged rows idempotently before swinging the active-build pointer.
//!
//! Transaction discipline (all proven against `TypeDB` 3.13.0):
//! - Content-keyed rows flush in bounded chunks ([`FLUSH_CHUNK`] rows per
//!   write transaction, each row probed by key before insert); ingestion
//!   never holds a whole repository in one transaction. One commit per row
//!   cost production ~300k commits and ~1 TB of WAL per full publication.
//! - Duplicate `@key` inserts fail with the `@unique` violation (`CNT9`),
//!   which the flush treats as already-present; a concurrent rival's commit
//!   fails with an isolation conflict (`STC2`). A chunk that meets either
//!   falls back to the per-row path, so first-write-wins is unchanged.
//! - Publication swings the pointer in a single write transaction that
//!   re-validates predecessor and generation live, so two publishers from
//!   the same generation cannot both succeed.
//! - Reads use read transactions; a mutation query in one fails server-side.
//! - Dropped transactions close without commit: staged-but-unflushed data
//!   never becomes visible because every consumer read pins the active
//!   build through the pointer.

use std::time::Duration;

use chaosbox_store::{MemoryStore, StoreError};
use typedb_driver::{
    Address, Addresses, Credentials, DriverOptions, DriverTlsConfig, TransactionOptions,
    TransactionType, TypeDBDriver,
};

use crate::common::{
    FLUSH_RETRIES, WRITE_TIMEOUT, driver_error, drain, is_conflict, is_unique_violation,
};
use crate::encode::str_lit;
/// Connection config re-exported for backend constructors.
pub use crate::common::TypeDbConfig;

/// Rows per write transaction in the chunked flush.
// ponytail: fixed chunk; make it configurable only if commit latency or the
// 30 s write timeout ever bites at this size (256 rows ≈ 512 local queries).
const FLUSH_CHUNK: usize = 256;

/// One idempotent, content-keyed row: `probe` matches an existing row by
/// its `@key`, `insert` writes it. Every attribute is a function of the key,
/// so a matching probe means the durable row is already byte-identical.
pub(super) struct KeyedRow {
    probe: String,
    insert: String,
}

impl KeyedRow {
    pub(super) fn new(type_name: &str, key_attr: &str, key: &str, insert: String) -> Self {
        Self {
            probe: format!(
                "match $x isa {type_name}, has {key_attr} {}; select $x; limit 1;",
                str_lit(key)
            ),
            insert,
        }
    }
}

mod chain;
mod flush;
mod flush_semantics;
mod impl_store;
mod memory;
mod publication;
mod replica;

// TypeDbConfig and shared driver plumbing live in [`crate::common`].

/// TypeDB-backed [`Store`]: staging validation in memory, durable rows in
/// `TypeDB`, publication through the active-build pointer.
pub struct TypeDbStore {
    config: TypeDbConfig,
    driver: Option<TypeDBDriver>,
    staging: MemoryStore,
}

// Flush methods hold no client across awaits beyond one short transaction;
// the driver is `!Clone`, so each call borrows it for the call duration.
impl TypeDbStore {
    /// A disconnected store with empty staging; connects lazily.
    #[must_use]
    pub fn new(config: TypeDbConfig) -> Self {
        Self {
            config,
            driver: None,
            staging: MemoryStore::default(),
        }
    }

    /// Connect the driver and ensure the database exists.
    async fn ensure_connected(&mut self) -> Result<(), StoreError> {
        if self.driver.is_none() {
            let address: Address = self
                .config
                .address
                .parse()
                .map_err(|e| StoreError::Connection(format!("bad address: {e}")))?;
            let driver = TypeDBDriver::new(
                Addresses::from_address(address),
                Credentials::new(&self.config.username, &self.config.password),
                DriverOptions::new(DriverTlsConfig::disabled()),
            )
            .await
            .map_err(driver_error)?;
            if !driver
                .databases()
                .contains(&self.config.database)
                .await
                .map_err(driver_error)?
            {
                driver
                    .databases()
                    .create(&self.config.database)
                    .await
                    .map_err(driver_error)?;
            }
            self.driver = Some(driver);
        }
        Ok(())
    }

    /// Apply the packaged schema. Idempotent: re-defining the identical
    /// schema commits cleanly, so `migrate` is safe to re-run.
    pub async fn migrate(&mut self) -> Result<(), StoreError> {
        self.ensure_connected().await?;
        let driver = self.driver.as_ref().expect("connected above");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Schema,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        drain(tx.query(crate::SCHEMA_TQL).await.map_err(driver_error)?)
            .await
            .map_err(|e| driver_error(*e))?;
        tx.commit().await.map_err(driver_error)?;
        Ok(())
    }

    /// Run one write pipeline and commit. Duplicate `@key` inserts surface
    /// the caller's choice: map them to already-present or propagate. The
    /// driver error is boxed across the await boundary (`result_large_err`);
    /// classification happens on the box before mapping.
    async fn write_one(&self, query: &str) -> Result<(), Box<typedb_driver::Error>> {
        let driver = self.driver.as_ref().expect("connected before flush");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(Box::new)?;
        let answer = tx.query(query).await.map_err(Box::new)?;
        drain(answer).await?;
        tx.commit().await.map_err(Box::new)?;
        Ok(())
    }

    /// Flush keyed rows in order, [`FLUSH_CHUNK`] rows per write transaction:
    /// skip every row whose probe already matches, insert the rest, commit
    /// once. Later rows may `match` earlier ones (memberships after their
    /// entities) because a transaction reads its own writes and chunks commit
    /// in order. Any failure on a chunk — duplicate key or isolation conflict
    /// from a rival, timeout, connection loss — replays that chunk through
    /// the per-row [`Self::insert_ignoring_duplicates`] path, which keeps the
    /// proven semantics and surfaces real errors unchanged.
    pub(super) async fn flush_keyed_rows(
        &self,
        rows: Box<dyn Iterator<Item = Result<KeyedRow, StoreError>> + Send + '_>,
    ) -> Result<(), StoreError> {
        let mut chunk = Vec::with_capacity(FLUSH_CHUNK);
        for row in rows {
            chunk.push(row?);
            if chunk.len() == FLUSH_CHUNK {
                self.flush_chunk(&chunk).await?;
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            self.flush_chunk(&chunk).await?;
        }
        Ok(())
    }

    async fn flush_chunk(&self, chunk: &[KeyedRow]) -> Result<(), StoreError> {
        if self.write_chunk(chunk).await.is_ok() {
            return Ok(());
        }
        for row in chunk {
            self.insert_ignoring_duplicates(&row.insert).await?;
        }
        Ok(())
    }

    async fn write_chunk(&self, chunk: &[KeyedRow]) -> Result<(), Box<typedb_driver::Error>> {
        use futures::TryStreamExt;
        use typedb_driver::answer::{ConceptRow, QueryAnswer};
        let driver = self.driver.as_ref().expect("connected before flush");
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(Box::new)?;
        for row in chunk {
            let present = match tx.query(&row.probe).await.map_err(Box::new)? {
                QueryAnswer::ConceptRowStream(_, stream) => {
                    let rows: Vec<ConceptRow> = stream.try_collect().await.map_err(Box::new)?;
                    !rows.is_empty()
                }
                _ => false,
            };
            if present {
                continue;
            }
            drain(tx.query(&row.insert).await.map_err(Box::new)?).await?;
        }
        tx.commit().await.map_err(Box::new)?;
        Ok(())
    }

    /// Insert-or-ignore: the `@unique` violation means a rival (or an
    /// earlier retry) already wrote this key. An isolation conflict (`STC2`)
    /// means a concurrent writer committed first: for these idempotent
    /// content-keyed rows the durable winner already exists, so treat it as
    /// success and let the caller re-read the winner. Publication pointer
    /// swings never use this helper (they must observe conflicts).
    async fn insert_ignoring_duplicates(&self, query: &str) -> Result<(), StoreError> {
        // Bounded transient retries on connection loss; conflicts and
        // constraint outcomes are decided, never retried.
        let mut attempt = 0;
        loop {
            match self.write_one(query).await {
                Ok(()) => return Ok(()),
                Err(e) if is_unique_violation(&e) => return Ok(()),
                Err(e) if is_conflict(&e) => return Ok(()),
                Err(e)
                    if matches!(&*e, typedb_driver::Error::Connection(_))
                        && attempt < FLUSH_RETRIES =>
                {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
                Err(e) => return Err(driver_error(*e)),
            }
        }
    }
}

impl Default for TypeDbStore {
    fn default() -> Self {
        Self::new(TypeDbConfig {
            address: "127.0.0.1:1729".to_owned(),
            username: "admin".to_owned(),
            password: String::new(),
            database: "chaosbox".to_owned(),
        })
    }
}
