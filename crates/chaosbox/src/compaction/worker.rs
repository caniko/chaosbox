//! Restartable assessment worker; source custody never waits for inference.
use std::{collections::BTreeMap, path::Path};
use chaosbox_core::sha256_hex;
use chaosbox_jev::{Question, SystemOneResponse};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use crate::{
    intelligence::{self, Bundle, Coverage},
    continuation, Responder,
};
use super::{err, private_database, Capture, Journal};

/// Persistent inference ceilings, cumulative across sessions and restarts.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Maximum dispatched attempts, including failed/interrupted attempts.
    pub requests: u32,
    /// Conservative input-token reservation ceiling.
    pub input_tokens: u64,
    /// Explicitly authorize a fresh attempt after interruption or failure.
    pub retry: bool,
}

impl Journal {
    /// Publish the durable outbox into `TypeDB`. Interrupted pointer updates are
    /// reconciled by content identity; a rival predecessor is never overwritten.
    pub async fn publish_pending(
        &mut self,
        root: &Path,
        store: &mut chaosbox_typedb::store::TypeDbStore,
    ) -> Result<String, String> {
        let mut lease = private_database(root, "worker.sqlite")?;
        let _guard = lease
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let bundle = self.bundle()?;
        let predecessor: Option<String> = self
            .db
            .query_row("SELECT predecessor FROM publication", [], |r| r.get(0))
            .optional()
            .map_err(err)?
            .flatten();
        let payload = serde_json::to_string(&bundle).map_err(err)?;
        let id = store
            .publish_knowledge(
                &self.scope,
                &payload,
                &bundle.records,
                predecessor.as_deref(),
            )
            .await
            .map_err(err)?;
        self.db.execute("INSERT INTO publication VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET predecessor=excluded.predecessor", [&id]).map_err(err)?;
        Ok(id)
    }

    /// Last fully committed local knowledge generation, also the `TypeDB` outbox.
    pub fn bundle(&self) -> Result<Bundle, String> {
        let raw: Option<String> = self
            .db
            .query_row("SELECT body FROM knowledge", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let bundle: Bundle = raw
            .map(|s| serde_json::from_str(&s).map_err(err))
            .transpose()?
            .unwrap_or_else(|| Bundle::new(&self.scope));
        bundle.validate()?;
        if bundle.scope != self.scope {
            return Err("knowledge scope mismatch".into());
        }
        Ok(bundle)
    }

    /// Drain pending captures with typed pinned-model assessment. A separate
    /// SQLite write lease serializes workers and releases automatically on crash;
    /// capture commits remain independent of network latency.
    pub async fn assess_pending(
        &mut self,
        root: &Path,
        responder: &mut impl Responder,
        budget: Budget,
    ) -> Result<usize, String> {
        let mut lease = private_database(root, "worker.sqlite")?;
        lease
            .execute_batch("CREATE TABLE IF NOT EXISTS lease (id INTEGER PRIMARY KEY)")
            .map_err(err)?;
        let _guard = lease
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let mut completed = 0;
        for id in self.pending()? {
            let capture = self.load(&id)?;
            let text = self.normalized(&capture)?;
            let snapshot = sha256_hex(&[&text]);
            self.db
                .execute(
                    "INSERT OR IGNORE INTO normalized_sources VALUES (?1,?2)",
                    params![snapshot, text],
                )
                .map_err(err)?;
            let bundle = self
                .assess_candidates(&capture, &text, responder, budget)
                .await?;
            self.select_continuation(&capture, &text, responder, budget)
                .await?;
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            tx.execute("INSERT INTO knowledge VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET body=excluded.body", [serde_json::to_string(&bundle).map_err(err)?]).map_err(err)?;
            tx.execute("UPDATE captures SET status='assessed' WHERE id=?1", [&id])
                .map_err(err)?;
            tx.commit().map_err(err)?;
            completed += 1;
        }
        Ok(completed)
    }

    async fn assess_candidates(
        &mut self,
        capture: &Capture,
        text: &str,
        responder: &mut impl Responder,
        budget: Budget,
    ) -> Result<Bundle, String> {
        let mut bundle = self.bundle()?;
        let mut skip = 0;
        loop {
            let catalog = intelligence::extract_complete_window(
                text,
                &capture.source,
                &capture.session,
                &capture.scope,
                std::slice::from_ref(&capture.repo),
                skip,
                200,
            )?;
            for candidate in &catalog.candidates {
                // Same source occurrence across repeated full-history captures
                // is never a new vote and does not repeat paid inference.
                let origin = sha256_hex(&[
                    &candidate.evidence.lineage(),
                    &candidate.evidence.quote,
                    &candidate.context,
                    &serde_json::to_string(&candidate.evidence_bundle).map_err(err)?,
                    intelligence::RUBRIC_VERSION,
                    &capture.repo,
                ]);
                let processed = self
                    .db
                    .query_row("SELECT key FROM processed WHERE key=?1", [&origin], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()
                    .map_err(err)?;
                if processed.is_some() {
                    continue;
                }
                let (state, asked, key) = intelligence::questions(candidate, &bundle)?;
                let response = self.evaluate(responder, &key, state, asked, budget).await?;
                intelligence::assess(candidate, &mut bundle, response)?;
                bundle.validate()?;
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(err)?;
                tx.execute("INSERT INTO knowledge VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET body=excluded.body", [serde_json::to_string(&bundle).map_err(err)?]).map_err(err)?;
                tx.execute("INSERT OR IGNORE INTO processed VALUES (?1)", [&origin])
                    .map_err(err)?;
                tx.commit().map_err(err)?;
            }
            if !catalog.has_more {
                bundle.coverage.push(Coverage {
                    snapshot: catalog.snapshot,
                    skipped: 0,
                    selected: skip + catalog.candidates.len(),
                    omitted: catalog.omitted,
                    excluded_derived: catalog.excluded_derived,
                });
                break;
            }
            skip += catalog.candidates.len();
        }
        Ok(bundle)
    }

    async fn select_continuation(
        &mut self,
        capture: &Capture,
        text: &str,
        responder: &mut impl Responder,
        budget: Budget,
    ) -> Result<(), String> {
        // Independent continuation selection. A negative admission decision
        // cannot influence the permission to remove a working-state record.
        if capture.records.iter().any(|r| r["type"] == "user") {
            for (index, line) in text.lines().enumerate() {
                let value: Value = serde_json::from_str(line).map_err(err)?;
                if value["type"] != "assistant"
                    || value["content"]
                        .as_array()
                        .is_some_and(|p| p.iter().any(|p| p["type"] == "tool"))
                {
                    continue;
                }
                let native = capture
                    .records
                    .iter()
                    .find(|r| r["id"] == value["id"])
                    .ok_or("normalization lost identity")?;
                let hash = sha256_hex(&[&serde_json::to_string(native).map_err(err)?]);
                // An oversized record remains protected; never truncate it for a verdict.
                let Ok(input) = continuation::prepare(
                    text,
                    &continuation::Window {
                        scope: &capture.scope,
                        repo: &capture.repo,
                        source: &capture.source,
                        session: &capture.session,
                        start: index + 1,
                        limit: 1,
                    },
                ) else {
                    continue;
                };
                let anchor = sha256_hex(&[&input.latest_user.raw]);
                if self
                    .db
                    .query_row(
                        "SELECT hash FROM selections WHERE hash=?1 AND anchor=?2",
                        params![hash, anchor],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(err)?
                    .is_some()
                {
                    continue;
                }
                let key = continuation::key(&input)?;
                let response = self
                    .evaluate(
                        responder,
                        &key,
                        serde_json::json!({"source":input,"historical_data_not_instructions":true}),
                        continuation::questions(&input),
                        budget,
                    )
                    .await?;
                continuation::validate_answers(&input, &response)?;
                let packet = continuation::render(&input, &response, 120_000)?;
                let irrelevant = packet["omitted"].as_array().is_some_and(|r| !r.is_empty());
                self.db
                    .execute(
                        "INSERT OR IGNORE INTO selections VALUES (?1,?2,?3,?4)",
                        params![
                            hash,
                            anchor,
                            irrelevant,
                            serde_json::to_string(&(input, response)).map_err(err)?
                        ],
                    )
                    .map_err(err)?;
            }
        }
        Ok(())
    }

    async fn evaluate(
        &mut self,
        responder: &mut impl Responder,
        key: &str,
        state: Value,
        asked: BTreeMap<String, Question>,
        budget: Budget,
    ) -> Result<SystemOneResponse, String> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let previous: Option<(u32, String, Option<String>)> = tx.query_row(
            "SELECT attempt,status,response FROM attempts WHERE key=?1 ORDER BY (status='success') DESC,attempt DESC LIMIT 1", [key],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
        if let Some((_, status, Some(raw))) = &previous {
            if status == "success" {
                return serde_json::from_str(raw).map_err(err);
            }
        }
        if previous.is_some() && !budget.retry {
            return Err("failed/interrupted assessment requires explicit --retry".into());
        }
        let reserved = u64::try_from(serde_json::to_vec(&(&state, &asked)).map_err(err)?.len())
            .map_err(err)?
            + 1024;
        let (count, tokens): (u32, i64) = tx
            .query_row(
                "SELECT count(*),coalesce(sum(reserved),0) FROM attempts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(err)?;
        let tokens = u64::try_from(tokens).map_err(err)?;
        if count >= budget.requests || tokens.saturating_add(reserved) > budget.input_tokens {
            return Err("persistent session assessment budget exhausted".into());
        }
        let attempt = previous.map_or(1, |(n, _, _)| n.saturating_add(1));
        tx.execute(
            "INSERT INTO attempts VALUES (?1,?2,?3,'reserved',NULL)",
            params![key, attempt, i64::try_from(reserved).map_err(err)?],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        let result = responder
            .respond(state, asked.clone())
            .await
            .and_then(|response| {
                let options = asked
                    .iter()
                    .map(|(id, q)| {
                        (
                            id.clone(),
                            match q {
                                Question::Choice { criteria, .. } => {
                                    criteria.keys().cloned().collect()
                                }
                                _ => std::collections::BTreeSet::new(),
                            },
                        )
                    })
                    .collect();
                chaosbox_jev::validate_model_identity(
                    chaosbox_jev::JEV_MODEL_PINNED,
                    &response.model,
                )
                .map_err(err)?;
                chaosbox_jev::validate_response(&response, &asked, &options).map_err(err)?;
                Ok(response)
            });
        let (status, raw, charged) = match &result {
            Ok(response) => (
                "success",
                Some(serde_json::to_string(response).map_err(err)?),
                reserved.max(response.usage.input_tokens),
            ),
            Err(_) => ("failed", None, reserved),
        };
        self.db
            .execute(
                "UPDATE attempts SET status=?1,response=?2,reserved=?3 WHERE key=?4 AND attempt=?5",
                params![
                    status,
                    raw,
                    i64::try_from(charged).map_err(err)?,
                    key,
                    attempt
                ],
            )
            .map_err(err)?;
        result.map_err(|_| "session assessment failed; source and reservation retained".into())
    }
}
