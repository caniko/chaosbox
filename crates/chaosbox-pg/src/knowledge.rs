//! Immutable private session-knowledge generations with a per-scope pointer,
//! shared by read-only readers and publishing stores.

use chaosbox_core::{intelligence::Intelligence, sha256_hex};
use chaosbox_store::StoreError;
use tokio_postgres::GenericClient;

use crate::common::{is_uncertain, pg_error, try_lock};
use crate::store::PgStore;

pub(crate) async fn current<C: GenericClient + Sync>(
    client: &C,
    scope: &str,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(client
        .query_opt(
            "SELECT b.build_id, b.body FROM knowledge_pointer p JOIN knowledge_builds b ON b.build_id = p.build_id AND b.scope = p.scope WHERE p.scope = $1",
            &[&scope],
        )
        .await
        .map_err(pg_error)?
        .map(|r| (r.get(0), r.get(1))))
}

pub(crate) async fn at<C: GenericClient + Sync>(
    client: &C,
    scope: &str,
    id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(client
        .query_opt(
            "SELECT body FROM knowledge_builds WHERE build_id = $1 AND scope = $2",
            &[&id, &scope],
        )
        .await
        .map_err(pg_error)?
        .map(|r| r.get(0)))
}

impl PgStore {
    /// The private scope's current knowledge generation: (id, export).
    pub async fn knowledge(&mut self, scope: &str) -> Result<Option<(String, String)>, StoreError> {
        current(&*self.client().await?, scope).await
    }

    /// One exact generation without consulting the pointer; the scope guard
    /// keeps historical ids inside their owner boundary.
    pub async fn knowledge_at(
        &mut self,
        scope: &str,
        id: &str,
    ) -> Result<Option<String>, StoreError> {
        at(&*self.client().await?, scope, id).await
    }

    /// Publish a validated export only if the pinned predecessor still owns
    /// this private scope. Exact retries return the active id unchanged.
    /// Statements become embedding inputs in the same transaction.
    pub async fn publish_knowledge(
        &mut self,
        scope: &str,
        payload: &str,
        records: &[Intelligence],
        expected: Option<&str>,
    ) -> Result<String, StoreError> {
        let invariant = |s: &str| StoreError::Invariant(s.into());
        if !scope.starts_with("private:") || scope.len() <= 8 || payload.len() > 64 * 1024 * 1024 {
            return Err(invariant("invalid private knowledge publication"));
        }
        let value: serde_json::Value =
            serde_json::from_str(payload).map_err(|_| invariant("invalid knowledge export"))?;
        if value["scope"] != scope
            || value["records"]
                != serde_json::to_value(records).map_err(|_| invariant("encode knowledge"))?
        {
            return Err(invariant("knowledge export does not match typed records"));
        }
        let ids: std::collections::BTreeSet<_> = records.iter().map(|r| &r.id).collect();
        if ids.len() != records.len()
            || records.iter().any(|r| {
                r.scope != scope
                    || r.evidence.is_empty()
                    || r.repositories.is_empty()
                    || r.contradicts.iter().any(|id| !ids.contains(id))
                    || r.supersedes.as_ref().is_some_and(|id| !ids.contains(id))
            })
        {
            return Err(invariant(
                "invalid knowledge scope, evidence or relationship",
            ));
        }
        let id = sha256_hex(&["session-knowledge-v1", scope, payload]);
        let live = self.knowledge(scope).await?;
        if live
            .as_ref()
            .is_some_and(|(active, body)| active == &id && body == payload)
        {
            return Ok(id);
        }
        if live.as_ref().map(|(active, _)| active.as_str()) != expected {
            return Err(invariant(
                "knowledge predecessor changed; reconcile before publishing",
            ));
        }
        let client = self.client().await?;
        let tx = client.transaction().await.map_err(pg_error)?;
        if !try_lock(&tx, &format!("knowledge:{scope}")).await?
            || current(&tx, scope).await?.as_ref().map(|(a, _)| a.as_str()) != expected
        {
            return Err(invariant(
                "concurrent knowledge publisher moved the pointer",
            ));
        }
        tx.execute(
            "INSERT INTO knowledge_builds (build_id, scope, body) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            &[&id, &scope, &payload],
        )
        .await
        .map_err(pg_error)?;
        if at(&tx, scope, &id).await?.as_deref() != Some(payload) {
            return Err(invariant("knowledge generation identity collision"));
        }
        let statements: Vec<&str> = records.iter().map(|r| r.statement.as_str()).collect();
        crate::store::put_texts(&tx, &statements).await?;
        tx.execute(
            "INSERT INTO knowledge_pointer (scope, build_id) VALUES ($1, $2) ON CONFLICT (scope) DO UPDATE SET build_id = EXCLUDED.build_id",
            &[&scope, &id],
        )
        .await
        .map_err(pg_error)?;
        match tx.commit().await {
            Ok(()) => Ok(id),
            Err(e) if is_uncertain(&e) => {
                self.forget_client();
                match self.knowledge(scope).await {
                    Ok(Some((active, _))) if active == id => Ok(id),
                    _ => Err(pg_error(e)),
                }
            }
            Err(e) => Err(pg_error(e)),
        }
    }
}
