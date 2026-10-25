//! Same-user peer replication: append-only events, collision-checked views
//! and a per-(user, scope) pointer advanced under an advisory lock.

use chaosbox_store::{validate_replica_view, validate_row, ReplicaRow, ReplicaStore, StoreError};
use tokio_postgres::GenericClient;

use super::PgStore;
use crate::common::{is_uncertain, pg_error, try_lock};

async fn current<C: GenericClient + Sync>(
    client: &C,
    user: &str,
    scope: &str,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(client
        .query_opt(
            "SELECT v.view_id, v.body FROM replica_pointer p JOIN replica_views v ON v.view_id = p.view_id AND v.replica_user = p.replica_user AND v.scope = p.scope WHERE p.replica_user = $1 AND p.scope = $2",
            &[&user, &scope],
        )
        .await
        .map_err(pg_error)?
        .map(|r| (r.get(0), r.get(1))))
}

#[async_trait::async_trait]
impl ReplicaStore for PgStore {
    async fn replica_put(&mut self, row: &ReplicaRow) -> Result<(), StoreError> {
        validate_row(row)?;
        let client = self.client().await?;
        client
            .execute(
                "INSERT INTO replica_events (replica_id, replica_user, scope, body) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                &[&row.id, &row.user, &row.scope, &row.body],
            )
            .await
            .map_err(pg_error)?;
        let stored = client
            .query_one(
                "SELECT replica_user, scope, body FROM replica_events WHERE replica_id = $1",
                &[&row.id],
            )
            .await
            .map_err(pg_error)?;
        if stored.get::<_, String>(0) != row.user
            || stored.get::<_, String>(1) != row.scope
            || stored.get::<_, String>(2) != row.body
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
        let limit = i64::try_from(limit).expect("bounded above");
        Ok(self
            .client()
            .await?
            .query(
                "SELECT replica_id, body FROM replica_events WHERE replica_user = $1 AND scope = $2 AND replica_id > $3 ORDER BY replica_id LIMIT $4",
                &[&user, &scope, &after, &limit],
            )
            .await
            .map_err(pg_error)?
            .iter()
            .map(|r| ReplicaRow {
                id: r.get(0),
                user: user.into(),
                scope: scope.into(),
                body: r.get(1),
            })
            .collect())
    }

    async fn replica_current(
        &mut self,
        user: &str,
        scope: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        current(&*self.client().await?, user, scope).await
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
        let client = self.client().await?;
        if current(&*client, user, scope)
            .await?
            .is_some_and(|(old, b)| old == id && b == body)
        {
            return Ok(());
        }
        let tx = client.transaction().await.map_err(pg_error)?;
        if !try_lock(&tx, &format!("replica:{}:{user}:{scope}", user.len())).await? {
            return Err(StoreError::Invariant("replica predecessor changed".into()));
        }
        tx.execute(
            "INSERT INTO replica_views (view_id, replica_user, scope, body) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            &[&id, &user, &scope, &body],
        )
        .await
        .map_err(pg_error)?;
        let stored = tx
            .query_one(
                "SELECT replica_user, scope, body FROM replica_views WHERE view_id = $1",
                &[&id],
            )
            .await
            .map_err(pg_error)?;
        if stored.get::<_, String>(0) != user
            || stored.get::<_, String>(1) != scope
            || stored.get::<_, String>(2) != body
        {
            return Err(StoreError::Invariant(
                "replica view identity collision".into(),
            ));
        }
        let live = current(&tx, user, scope).await?;
        if live.as_ref().map(|(active, _)| active.as_str()) != expected {
            return Err(StoreError::Invariant("replica predecessor changed".into()));
        }
        tx.execute(
            "INSERT INTO replica_pointer (replica_user, scope, view_id) VALUES ($1, $2, $3) ON CONFLICT (replica_user, scope) DO UPDATE SET view_id = EXCLUDED.view_id",
            &[&user, &scope, &id],
        )
        .await
        .map_err(pg_error)?;
        match tx.commit().await {
            Ok(()) => Ok(()),
            Err(e) if is_uncertain(&e) => {
                self.client = None;
                match self.replica_current(user, scope).await {
                    Ok(Some((active, b))) if active == id && b == body => Ok(()),
                    _ => Err(pg_error(e)),
                }
            }
            Err(e) => Err(pg_error(e)),
        }
    }

    async fn put_texts(&mut self, bodies: &[&str]) -> Result<(), StoreError> {
        let tx = self.client().await?.transaction().await.map_err(pg_error)?;
        super::put_texts(&tx, bodies).await?;
        tx.commit().await.map_err(pg_error)
    }

    async fn rank_texts(
        &mut self,
        model: &str,
        query: &[f32],
        shas: &[String],
    ) -> Result<Vec<(String, f64)>, StoreError> {
        crate::reader::rank_texts(&*self.client().await?, model, query, shas).await
    }
}
