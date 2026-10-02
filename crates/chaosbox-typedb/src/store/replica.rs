//! Same-user event persistence; publication guards execute in the write transaction.
use chaosbox_core::sha256_hex;
use chaosbox_store::{ReplicaRow, ReplicaStore, StoreError, validate_row, validate_replica_view};
use futures::TryStreamExt;
use typedb_driver::{TransactionType, answer::QueryAnswer};
use crate::{
    common::{col_string, read_rows, drain, driver_error, is_conflict, is_unique_violation},
    encode::str_lit,
};
use super::TypeDbStore;

impl TypeDbStore {
    // Consumers connect to an existing database only: a read must not create it.
    async fn replica_connect_existing(&mut self) -> Result<(), StoreError> {
        if self.driver.is_none() {
            let address = self
                .config
                .address
                .parse()
                .map_err(|e| StoreError::Connection(format!("bad address: {e}")))?;
            let driver = typedb_driver::TypeDBDriver::new(
                typedb_driver::Addresses::from_address(address),
                typedb_driver::Credentials::new(&self.config.username, &self.config.password),
                typedb_driver::DriverOptions::new(typedb_driver::DriverTlsConfig::disabled()),
            )
            .await
            .map_err(driver_error)?;
            if !driver
                .databases()
                .contains(&self.config.database)
                .await
                .map_err(driver_error)?
            {
                return Err(StoreError::NotFound(
                    "replica database missing; run db migrate".into(),
                ));
            }
            self.driver = Some(driver);
        }
        Ok(())
    }
    async fn replica_insert(&self, query: &str) -> Result<(), StoreError> {
        for _ in 0..3 {
            match self.write_one(query).await {
                Ok(()) => return Ok(()),
                Err(e) if is_unique_violation(&e) => return Ok(()),
                Err(e) if is_conflict(&e) => (),
                Err(e) => return Err(driver_error(*e)),
            }
        }
        Err(StoreError::Invariant(
            "replication staging contention; retry".into(),
        ))
    }
}

#[async_trait::async_trait]
impl ReplicaStore for TypeDbStore {
    async fn replica_put(&mut self, row: &ReplicaRow) -> Result<(), StoreError> {
        validate_row(row)?;
        self.replica_connect_existing().await?;
        self.replica_insert(&format!("insert $e isa replica-event, has replica-id {}, has replica-user {}, has scope {}, has raw-envelope {};",
            str_lit(&row.id), str_lit(&row.user), str_lit(&row.scope), str_lit(&row.body))).await?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("not connected".into()))?;
        let rows = read_rows(driver, &self.config.database, &format!("match $e isa replica-event, has replica-id {}, has replica-user $u, has scope $s, has raw-envelope $b; select $u, $s, $b;", str_lit(&row.id)), &["u","s","b"]).await?;
        let stored = rows
            .first()
            .ok_or_else(|| StoreError::Invariant("replica insert not durable".into()))?;
        if col_string(stored, "u")? != row.user
            || col_string(stored, "s")? != row.scope
            || col_string(stored, "b")? != row.body
        {
            return Err(StoreError::Invariant(
                "replication identity collision".into(),
            ));
        }
        Ok(())
    }
    async fn replica_rows(
        &mut self,
        user: &str,
        scope: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<ReplicaRow>, StoreError> {
        if !(1..=200).contains(&limit) {
            return Err(StoreError::Invariant("replica page limit 1..200".into()));
        }
        self.replica_connect_existing().await?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("not connected".into()))?;
        let q = format!(
            "match $e isa replica-event, has replica-user {}, has scope {}, has replica-id $id, has raw-envelope $b; $id > {}; select $id, $b; sort $id asc; limit {limit};",
            str_lit(user),
            str_lit(scope),
            str_lit(after)
        );
        read_rows(driver, &self.config.database, &q, &["id", "b"])
            .await?
            .iter()
            .map(|r| {
                Ok(ReplicaRow {
                    id: col_string(r, "id")?,
                    user: user.into(),
                    scope: scope.into(),
                    body: col_string(r, "b")?,
                })
            })
            .collect()
    }
    async fn replica_current(
        &mut self,
        user: &str,
        scope: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        self.replica_connect_existing().await?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("not connected".into()))?;
        let q = format!(
            "match $p isa replica-pointer, has replica-key {}, has replica-view-id $id; $v isa replica-view, has replica-view-id $id, has replica-user {}, has scope {}, has raw-envelope $b; select $id, $b;",
            str_lit(&sha256_hex(&[user, scope])),
            str_lit(user),
            str_lit(scope)
        );
        read_rows(driver, &self.config.database, &q, &["id", "b"])
            .await?
            .first()
            .map(|r| Ok((col_string(r, "id")?, col_string(r, "b")?)))
            .transpose()
    }
    async fn replica_publish(
        &mut self,
        user: &str,
        scope: &str,
        id: &str,
        body: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError> {
        validate_replica_view(user, scope, id, body)?;
        if self
            .replica_current(user, scope)
            .await?
            .is_some_and(|(old, b)| old == id && b == body)
        {
            return Ok(());
        }
        self.replica_insert(&format!("insert $v isa replica-view, has replica-view-id {}, has replica-user {}, has scope {}, has raw-envelope {};",str_lit(id),str_lit(user),str_lit(scope),str_lit(body))).await?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("not connected".into()))?;
        let rows = read_rows(driver,&self.config.database,&format!("match $v isa replica-view, has replica-view-id {}, has replica-user $u, has scope $s, has raw-envelope $b; select $u, $s, $b;",str_lit(id)),&["u","s","b"]).await?;
        let row = rows
            .first()
            .ok_or_else(|| StoreError::Invariant("replica view insert not durable".into()))?;
        if col_string(row, "u")? != user
            || col_string(row, "s")? != scope
            || col_string(row, "b")? != body
        {
            return Err(StoreError::Invariant(
                "replica view identity collision".into(),
            ));
        }
        let tx = driver
            .transaction(&self.config.database, TransactionType::Write)
            .await
            .map_err(driver_error)?;
        let key = sha256_hex(&[user, scope]);
        let guard = match expected {
            Some(pred) => format!(
                "match $p isa replica-pointer, has replica-key {}, has replica-view-id {}; select $p;",
                str_lit(&key),
                str_lit(pred)
            ),
            None => format!(
                "match $v isa replica-view, has replica-view-id {}; not {{ $p isa replica-pointer, has replica-key {}; }}; select $v;",
                str_lit(id),
                str_lit(&key)
            ),
        };
        let found = match tx.query(&guard).await.map_err(driver_error)? {
            QueryAnswer::ConceptRowStream(_, s) => !s
                .try_collect::<Vec<_>>()
                .await
                .map_err(driver_error)?
                .is_empty(),
            _ => return Err(StoreError::Query("replica guard expected rows".into())),
        };
        if !found {
            return Err(StoreError::Invariant("replica predecessor changed".into()));
        }
        let query = if expected.is_some() {
            format!(
                "match $p isa replica-pointer, has replica-key {}; update $p has replica-view-id {};",
                str_lit(&key),
                str_lit(id)
            )
        } else {
            format!(
                "insert $p isa replica-pointer, has replica-key {}, has replica-view-id {};",
                str_lit(&key),
                str_lit(id)
            )
        };
        drain(tx.query(&query).await.map_err(driver_error)?)
            .await
            .map_err(|e| driver_error(*e))?;
        match tx.commit().await {
            Ok(()) => Ok(()),
            Err(e) => {
                if self
                    .replica_current(user, scope)
                    .await?
                    .is_some_and(|(active, b)| active == id && b == body)
                {
                    Ok(())
                } else if is_conflict(&e) || is_unique_violation(&e) {
                    Err(StoreError::Invariant("replica predecessor changed".into()))
                } else {
                    Err(driver_error(e))
                }
            }
        }
    }
}
