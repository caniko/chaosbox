//! Immutable private intelligence generations and atomic predecessor guards.
use chaosbox_core::{intelligence::Intelligence, sha256_hex};
use chaosbox_store::StoreError;
use futures::TryStreamExt;
use typedb_driver::{TransactionOptions, TransactionType};
use crate::{
    common::{
        col_string, drain, driver_error, is_conflict, is_unique_violation, read_rows, WRITE_TIMEOUT,
    },
    encode::{str_lit, int_lit},
};
use super::TypeDbStore;

impl TypeDbStore {
    async fn knowledge_insert(&self, query: &str) -> Result<(), StoreError> {
        for _ in 0..3 {
            match self.write_one(query).await {
                Ok(()) => return Ok(()),
                Err(error) if is_unique_violation(&error) => return Ok(()),
                // Isolation conflicts can concern a different staged row; they
                // do not prove that this insert became durable.
                Err(error) if is_conflict(&error) => (),
                Err(error) => return Err(driver_error(*error)),
            }
        }
        Err(StoreError::Invariant(
            "knowledge staging contention; retry publication".into(),
        ))
    }
    /// Pin the private scope's complete knowledge export in one read transaction.
    /// Export contains typed assessment receipts; source bytes stay in custody.
    pub async fn knowledge(&mut self, scope: &str) -> Result<Option<(String, String)>, StoreError> {
        self.ensure_connected().await?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("not connected".into()))?;
        let rows = read_rows(driver, &self.config.database, &format!("match $p isa session-knowledge-pointer, has memory-scope {}, has memory-build-id $id; $b isa session-knowledge-build, has memory-build-id $id, has memory-scope {}, has raw-envelope $body; select $id, $body;", str_lit(scope), str_lit(scope)), &["id","body"]).await?;
        rows.first()
            .map(|r| Ok((col_string(r, "id")?, col_string(r, "body")?)))
            .transpose()
    }

    /// Stage typed intelligence/evidence/lifecycle relationships, then atomically
    /// publish only if the pinned predecessor still owns this private scope.
    /// Exact retries reconcile an already-active generation without duplication.
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
        let current = self.knowledge(scope).await?;
        if current
            .as_ref()
            .is_some_and(|(active, body)| active == &id && body == payload)
        {
            return Ok(id);
        }
        if current.as_ref().map(|(active, _)| active.as_str()) != expected {
            return Err(invariant(
                "knowledge predecessor changed; reconcile before publishing",
            ));
        }
        self.stage_knowledge(scope, &id, payload, records).await?;
        self.advance_knowledge(scope, &id, expected).await?;
        Ok(id)
    }

    async fn stage_knowledge(
        &self,
        scope: &str,
        id: &str,
        payload: &str,
        records: &[Intelligence],
    ) -> Result<(), StoreError> {
        let invariant = |s: &str| StoreError::Invariant(s.into());
        self.knowledge_insert(&format!("insert $b isa session-knowledge-build, has memory-build-id {}, has memory-scope {}, has raw-envelope {};", str_lit(id), str_lit(scope), str_lit(payload))).await?;
        let item_key = |intel: &str| sha256_hex(&[id, intel]);
        for record in records {
            let key = item_key(&record.id);
            self.knowledge_insert(&format!("insert $i isa session-intelligence, has memory-item-id {}, has intelligence-id {}, has memory-scope {}, has kind {}, has state {}, has text {}, has raw-envelope {};",
                str_lit(&key), str_lit(&record.id), str_lit(scope), str_lit(&format!("{:?}",record.kind)), str_lit(&format!("{:?}",record.status)), str_lit(&record.statement), str_lit(&serde_json::to_string(record).map_err(|_| invariant("encode record"))?))).await?;
            self.knowledge_insert(&format!("match $b isa session-knowledge-build, has memory-build-id {}; $i isa session-intelligence, has memory-item-id {}; insert (build: $b, item: $i) isa session-knowledge-membership, has memory-link-id {};", str_lit(id), str_lit(&key), str_lit(&sha256_hex(&["member",id,&key])))).await?;
            for evidence in &record.evidence {
                let body =
                    serde_json::to_string(evidence).map_err(|_| invariant("encode evidence"))?;
                let ekey = sha256_hex(&[scope, &body]);
                self.knowledge_insert(&format!("insert $e isa session-source-evidence, has memory-evidence-id {}, has memory-scope {}, has native-source {}, has native-session {}, has native-message {}, has snapshot-id {}, has json-pointer {}, has start-line {}, has text {}, has raw-envelope {};",
                    str_lit(&ekey),str_lit(scope),str_lit(&evidence.source),str_lit(&evidence.session),str_lit(&evidence.message),str_lit(&evidence.snapshot),str_lit(&evidence.pointer),int_lit(i64::try_from(evidence.line).map_err(|_| invariant("invalid evidence line"))?),str_lit(&evidence.quote),str_lit(&body))).await?;
                self.knowledge_insert(&format!("match $i isa session-intelligence, has memory-item-id {}; $e isa session-source-evidence, has memory-evidence-id {}; insert (item: $i, evidence: $e) isa session-support, has memory-link-id {};",str_lit(&key),str_lit(&ekey),str_lit(&sha256_hex(&["support",&key,&ekey])))).await?;
            }
        }
        for record in records {
            for other in &record.contradicts {
                self.knowledge_insert(&format!("match $a isa session-intelligence, has memory-item-id {}; $b isa session-intelligence, has memory-item-id {}; insert (lhs: $a, rhs: $b) isa session-conflict, has memory-link-id {};",str_lit(&item_key(&record.id)),str_lit(&item_key(other)),str_lit(&sha256_hex(&["conflict",id,&record.id,other])))).await?;
            }
            if let Some(other) = &record.supersedes {
                self.knowledge_insert(&format!("match $a isa session-intelligence, has memory-item-id {}; $b isa session-intelligence, has memory-item-id {}; insert (newer: $a, older: $b) isa session-supersession, has memory-link-id {};",str_lit(&item_key(&record.id)),str_lit(&item_key(other)),str_lit(&sha256_hex(&["supersession",id,&record.id,other])))).await?;
            }
        }
        Ok(())
    }

    async fn advance_knowledge(
        &mut self,
        scope: &str,
        id: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError> {
        let invariant = |s: &str| StoreError::Invariant(s.into());
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| StoreError::Connection("not connected".into()))?;
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Write,
                TransactionOptions::new().transaction_timeout(WRITE_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        let guard = match expected {
            Some(pred) => format!(
                "match $p isa session-knowledge-pointer, has memory-scope {}, has memory-build-id {}; select $p;",
                str_lit(scope),
                str_lit(pred)
            ),
            None => format!(
                "match $b isa session-knowledge-build, has memory-build-id {}; not {{ $p isa session-knowledge-pointer, has memory-scope {}; }}; select $b;",
                str_lit(id),
                str_lit(scope)
            ),
        };
        let guarded = match tx.query(&guard).await.map_err(driver_error)? {
            typedb_driver::answer::QueryAnswer::ConceptRowStream(_, stream) => !stream
                .try_collect::<Vec<_>>()
                .await
                .map_err(driver_error)?
                .is_empty(),
            _ => return Err(StoreError::Query("knowledge guard expected rows".into())),
        };
        if !guarded {
            return Err(invariant(
                "concurrent knowledge publisher moved the pointer",
            ));
        }
        let query = if expected.is_some() {
            format!(
                "match $p isa session-knowledge-pointer, has memory-scope {}; update $p has memory-build-id {};",
                str_lit(scope),
                str_lit(id)
            )
        } else {
            format!(
                "insert $p isa session-knowledge-pointer, has memory-scope {}, has memory-build-id {};",
                str_lit(scope),
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
                    .knowledge(scope)
                    .await?
                    .as_ref()
                    .is_some_and(|(active, _)| active == id)
                {
                    Ok(())
                } else {
                    Err(driver_error(e))
                }
            }
        }
    }
}
