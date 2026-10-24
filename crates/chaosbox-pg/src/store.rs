//! Driver-backed PostgreSQL [`Store`]: staging validation in memory, durable
//! rows in PostgreSQL, publication through the active-build pointer.
//!
//! - Decisions and inferences are durable-first: one statement each, then the
//!   durable winner is staged, so a lost race never stages a loser.
//! - Publication runs in ONE transaction holding a per-repository advisory
//!   lock: live predecessor/generation checks, the build row, entities,
//!   memberships, relationships with sealed evidence, audit rows, then the
//!   pointer swing. A rival either loses the lock or sees the moved pointer.
//! - Published rows are inserted `ON CONFLICT DO NOTHING` and never updated.
//! - A failed publication leaves this instance's staging ahead of live state;
//!   retry with a fresh store.

use chaosbox_core::{
    entity_kind_name, relation_type_name, Candidate, Claim, Decision, DecisionOutcome, Entity,
    EntityKind, Evidence, GraphBuild, InferenceRecord, RawAnswer, Relation, SnapshotFile,
};
use chaosbox_store::{MemoryStore, StagedDataRef, Store, StoreError, StoreStats};
use tokio_postgres::{Client, Transaction};

pub use crate::common::PgConfig;
use crate::common::{
    clean, connect, decision_key, fold, insert_rows, is_uncertain, json, pg_error, text_sha256,
    try_lock, vector_literal,
};

mod replica;

const CONCURRENT_PUBLISHER: &str =
    "concurrent publisher moved the pointer; build staged but not activated";

/// PostgreSQL-backed [`Store`].
pub struct PgStore {
    config: PgConfig,
    client: Option<Client>,
    staging: MemoryStore,
}

impl Default for PgStore {
    fn default() -> Self {
        Self::new(
            PgConfig::new("host=/run/postgresql dbname=chaosbox")
                .expect("static unix-socket config"),
        )
    }
}

impl PgStore {
    /// A disconnected store with empty staging; connects lazily.
    #[must_use]
    pub fn new(config: PgConfig) -> Self {
        Self {
            config,
            client: None,
            staging: MemoryStore::default(),
        }
    }

    /// Connect once; the database must already exist (never created here).
    pub(crate) async fn client(&mut self) -> Result<&mut Client, StoreError> {
        if self.client.as_ref().is_none_or(Client::is_closed) {
            self.client = Some(connect(&self.config).await?);
        }
        Ok(self.client.as_mut().expect("connected above"))
    }

    /// Drop the session after an uncertain commit; the next call reconnects.
    pub(crate) fn forget_client(&mut self) {
        self.client = None;
    }

    /// Apply the packaged schema in one transaction, serialized across
    /// concurrent migrators. Idempotent.
    pub async fn migrate(&mut self) -> Result<(), StoreError> {
        let client = self.client().await?;
        let tx = client.transaction().await.map_err(pg_error)?;
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('chaosbox:migrate', 0))",
            &[],
        )
        .await
        .map_err(pg_error)?;
        tx.batch_execute(crate::SCHEMA_SQL)
            .await
            .map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)
    }

    /// Up to `limit` stored embedding inputs that have no vector for `model`,
    /// in key order.
    pub async fn pending_texts(
        &mut self,
        model: &str,
        limit: i64,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self
            .client()
            .await?
            .query(
                "SELECT t.text_sha256, t.body FROM texts t WHERE NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.model = $1 AND e.text_sha256 = t.text_sha256) ORDER BY t.text_sha256 LIMIT $2",
                &[&model, &limit],
            )
            .await
            .map_err(pg_error)?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Store unit vectors for stored texts; existing vectors are kept.
    pub async fn put_embeddings(
        &mut self,
        model: &str,
        vectors: &[(String, Vec<f32>)],
    ) -> Result<(), StoreError> {
        let rows = vectors
            .iter()
            .map(|(sha, vector)| {
                if vector.len() != crate::EMBED_DIMS {
                    return Err(StoreError::Invariant(format!(
                        "embedding width {} != {}",
                        vector.len(),
                        crate::EMBED_DIMS
                    )));
                }
                Ok([
                    Some(model.to_owned()),
                    Some(sha.clone()),
                    Some(vector_literal(vector)?),
                ])
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let tx = self.client().await?.transaction().await.map_err(pg_error)?;
        insert_rows(
            &tx,
            "INSERT INTO embeddings (model, text_sha256, vec) SELECT m, s, v::halfvec FROM UNNEST($1::text[], $2::text[], $3::text[]) AS t(m, s, v) ON CONFLICT DO NOTHING",
            rows,
        )
        .await?;
        tx.commit().await.map_err(pg_error)
    }

    async fn read_decision(&mut self, key: &str) -> Result<Option<Decision>, StoreError> {
        let row = self
            .client()
            .await?
            .query_opt(
                "SELECT body FROM decisions WHERE decision_key = $1",
                &[&key],
            )
            .await
            .map_err(pg_error)?;
        row.map(|r| decode(&r.get::<_, String>(0), "decision"))
            .transpose()
    }

    async fn read_inference(
        &mut self,
        reuse_key: &str,
    ) -> Result<Option<InferenceRecord>, StoreError> {
        let row = self
            .client()
            .await?
            .query_opt(
                "SELECT body FROM inferences WHERE reuse_key = $1",
                &[&reuse_key],
            )
            .await
            .map_err(pg_error)?;
        row.map(|r| decode(&r.get::<_, String>(0), "inference"))
            .transpose()
    }

    async fn live_pointer(&mut self, repo: &str) -> Result<Option<String>, StoreError> {
        let row = self
            .client()
            .await?
            .query_opt(
                "SELECT build_id FROM active_pointer WHERE repo = $1",
                &[&repo],
            )
            .await
            .map_err(pg_error)?;
        Ok(row.map(|r| r.get(0)))
    }

    /// The guarded publication transaction; see the module docs.
    async fn publish_tx(
        &mut self,
        build: GraphBuild,
        expected: Option<String>,
        digest: &str,
    ) -> Result<(), StoreError> {
        let generation = i64::try_from(build.generation)
            .map_err(|_| StoreError::Invariant("generation overflows i64".into()))?;
        self.client().await?;
        let Self {
            client, staging, ..
        } = self;
        let client = client.as_mut().expect("connected above");
        let tx = client.transaction().await.map_err(pg_error)?;
        if !try_lock(&tx, &format!("build:{}", build.repo)).await? {
            return Err(StoreError::Invariant(CONCURRENT_PUBLISHER.into()));
        }
        let live = tx
            .query_opt(
                "SELECT b.build_id, b.generation, b.digest FROM active_pointer p JOIN graph_builds b ON b.build_id = p.build_id WHERE p.repo = $1",
                &[&build.repo],
            )
            .await
            .map_err(pg_error)?
            .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1), r.get::<_, String>(2)));
        match (&live, &expected) {
            (None, None) => {}
            (Some((active, _, stored)), _) if *active == build.id => {
                // An exact retry must describe the durable sealed version.
                return if stored == digest {
                    Ok(())
                } else {
                    Err(StoreError::Invariant(
                        "build identity reused with a different or unsealed version".into(),
                    ))
                };
            }
            (Some((active, live_gen, _)), Some(pred))
                if pred == active && generation > *live_gen => {}
            (Some((active, _, _)), Some(pred)) if pred == active => {
                return Err(StoreError::Invariant(
                    "older worker cannot replace newer build".into(),
                ));
            }
            (Some((active, live_gen, _)), pred) => {
                let pred = pred.as_deref().unwrap_or("None");
                return Err(StoreError::Invariant(format!(
                    "predecessor mismatch: expected {pred}, live active is {active} (gen {live_gen})"
                )));
            }
            (None, Some(pred)) => {
                return Err(StoreError::Invariant(format!(
                    "expected predecessor {pred} but no live active build"
                )));
            }
        }
        // Local invariants (cross-build edges, generations, sealing). A fresh
        // process has empty staging: its predecessor was validated live above.
        let staging_pred = staging.active_ref(&build.repo).and(expected);
        let build_id = build.id.clone();
        staging.publish(build, staging_pred).await?;
        let build = staging
            .get_ref(&build_id)
            .ok_or_else(|| StoreError::NotFound(build_id.clone()))?;
        write_build(&tx, staging, build, digest).await?;
        write_chain(&tx, staging.staged()).await?;
        tx.execute(
            "INSERT INTO active_pointer (repo, build_id) VALUES ($1, $2) ON CONFLICT (repo) DO UPDATE SET build_id = EXCLUDED.build_id, updated = now()",
            &[&build.repo, &build.id],
        )
        .await
        .map_err(pg_error)?;
        let repo = build.repo.clone();
        match tx.commit().await {
            Ok(()) => Ok(()),
            Err(e) if is_uncertain(&e) => {
                // Reconcile from durable state on a fresh session.
                self.client = None;
                match self.live_pointer(&repo).await {
                    Ok(Some(active)) if active == build_id => Ok(()),
                    _ => Err(pg_error(e)),
                }
            }
            Err(e) => Err(pg_error(e)),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(body: &str, what: &str) -> Result<T, StoreError> {
    serde_json::from_str(body).map_err(|e| StoreError::Query(format!("invalid stored {what}: {e}")))
}

/// Embedding input for an entity: its qualified name and bounded declaration
/// text, for the kinds whose text is a declaration or a section.
pub(crate) fn embed_input(e: &Entity) -> Option<String> {
    let text = e.text.as_deref()?;
    matches!(
        e.kind,
        EntityKind::Definition | EntityKind::Symbol | EntityKind::Heading
    )
    .then(|| format!("{}\n{text}", e.qualified_name))
}

/// Insert content-addressed texts in key order (consistent lock order).
pub(crate) async fn put_texts(tx: &Transaction<'_>, bodies: &[&str]) -> Result<(), StoreError> {
    let texts: std::collections::BTreeMap<String, String> = bodies
        .iter()
        .map(|body| (text_sha256(body), clean(body)))
        .collect();
    let rows = texts
        .into_iter()
        .map(|(sha, body)| [Some(sha), Some(body)])
        .collect();
    insert_rows(
        tx,
        "INSERT INTO texts (text_sha256, body) SELECT * FROM UNNEST($1::text[], $2::text[]) ON CONFLICT DO NOTHING",
        rows,
    )
    .await
}

/// Insert the build header once; an existing header must be this version.
async fn write_header(
    tx: &Transaction<'_>,
    build: &GraphBuild,
    digest: &str,
) -> Result<(), StoreError> {
    let navigation =
        json(&chaosbox_core::navigation::summarize_build(build).map_err(StoreError::Query)?)?;
    let coverage = build.coverage.as_ref().map(json).transpose()?;
    let count = |n: usize| {
        i64::try_from(n).map_err(|_| StoreError::Invariant("count overflows i64".into()))
    };
    let generation = i64::try_from(build.generation)
        .map_err(|_| StoreError::Invariant("generation overflows i64".into()))?;
    tx.execute(
        "INSERT INTO graph_builds (build_id, repo, generation, predecessor, snapshot_ids, coverage, navigation, digest, node_count, edge_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING",
        &[
            &build.id,
            &build.repo,
            &generation,
            &build.predecessor,
            &build.snapshot_ids,
            &coverage,
            &navigation,
            &digest,
            &count(build.nodes.len())?,
            &count(build.edges.len())?,
        ],
    )
    .await
    .map_err(pg_error)?;
    let stored = tx
        .query_one(
            "SELECT repo, digest FROM graph_builds WHERE build_id = $1",
            &[&build.id],
        )
        .await
        .map_err(pg_error)?;
    if stored.get::<_, String>(0) != build.repo || stored.get::<_, String>(1) != digest {
        return Err(StoreError::Invariant(
            "build identity reused with a different or unsealed version".into(),
        ));
    }
    Ok(())
}

/// Build header, entities, embedding texts, memberships and relationships.
async fn write_build(
    tx: &Transaction<'_>,
    staging: &MemoryStore,
    build: &GraphBuild,
    digest: &str,
) -> Result<(), StoreError> {
    write_header(tx, build, digest).await?;
    let mut nodes: Vec<&Entity> = build.nodes.values().collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let inputs: Vec<Option<String>> = nodes.iter().map(|e| embed_input(e)).collect();
    let bodies: Vec<&str> = inputs.iter().flatten().map(String::as_str).collect();
    put_texts(tx, &bodies).await?;
    let entity_rows = nodes
        .iter()
        .zip(&inputs)
        .map(|(e, input)| {
            Ok([
                Some(e.id.clone()),
                Some(entity_kind_name(&e.kind)),
                Some(clean(&e.repo)),
                Some(clean(&e.snapshot)),
                Some(clean(&e.file)),
                Some(clean(&e.name)),
                Some(clean(&e.qualified_name)),
                Some(fold(&e.name)),
                Some(fold(&e.qualified_name)),
                Some(json(&e.span)?),
                e.compiler.as_ref().map(json).transpose()?,
                input.as_deref().map(text_sha256),
            ])
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    insert_rows(
        tx,
        "INSERT INTO entities (entity_id, kind, repo, snapshot_id, file, name, qualified_name, name_fold, qn_fold, span, compiler, text_sha256) SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], $8::text[], $9::text[], $10::text[], $11::text[], $12::text[]) ON CONFLICT DO NOTHING",
        entity_rows,
    )
    .await?;
    insert_rows(
        tx,
        "INSERT INTO node_members (build_id, entity_id) SELECT * FROM UNNEST($1::text[], $2::text[]) ON CONFLICT DO NOTHING",
        nodes
            .iter()
            .map(|e| [Some(build.id.clone()), Some(e.id.clone())])
            .collect(),
    )
    .await?;
    let mut edges: Vec<&Relation> = build.edges.values().collect();
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    let edge_rows = edges
        .iter()
        .map(|r| {
            let scope = serde_json::to_value(&r.scope)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("{:?}", r.scope));
            Ok([
                Some(build.id.clone()),
                Some(r.id.clone()),
                Some(relation_type_name(&r.rel_type)),
                Some(r.from.clone()),
                Some(r.to.clone()),
                Some(scope),
                Some(json(staging.sealed_evidence(&build.id, &r.id)?)?),
            ])
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    insert_rows(
        tx,
        "INSERT INTO relationships (build_id, rel_id, rel_type, from_id, to_id, scope, sealed_evidence) SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[]) ON CONFLICT DO NOTHING",
        edge_rows,
    )
    .await
}

/// Audit rows behind the publication. Decisions and inferences are already
/// durable (written first by `put_*`) and are never rewritten here: a replay
/// could roll back a newer decision written by another process.
async fn write_chain(tx: &Transaction<'_>, staged: StagedDataRef<'_>) -> Result<(), StoreError> {
    let keyed = |table: &str, key: &str| {
        format!("INSERT INTO {table} ({key}, body) SELECT * FROM UNNEST($1::text[], $2::text[]) ON CONFLICT DO NOTHING")
    };
    insert_rows(
        tx,
        &keyed("runs", "run_id"),
        staged
            .runs
            .iter()
            .map(|(id, (repo, snapshot))| {
                Ok([
                    Some(id.clone()),
                    Some(json(
                        &serde_json::json!({"repo": repo, "snapshot": snapshot}),
                    )?),
                ])
            })
            .collect::<Result<Vec<_>, StoreError>>()?,
    )
    .await?;
    insert_rows(
        tx,
        &keyed("candidate_sets", "set_id"),
        staged
            .sets
            .iter()
            .map(|(id, (run, catalog, rubric))| {
                Ok([
                    Some(id.clone()),
                    Some(json(&serde_json::json!({"run": run, "catalog_digest": catalog, "rubric_version": rubric}))?),
                ])
            })
            .collect::<Result<Vec<_>, StoreError>>()?,
    )
    .await?;
    insert_rows(
        tx,
        &keyed("candidates", "candidate_id"),
        staged
            .candidates
            .iter()
            .map(|(id, (set, c)): (&String, &(String, Candidate))| {
                Ok([
                    Some(id.clone()),
                    Some(json(&serde_json::json!({"set_id": set, "candidate": c}))?),
                ])
            })
            .collect::<Result<Vec<_>, StoreError>>()?,
    )
    .await?;
    insert_rows(
        tx,
        "INSERT INTO file_versions (snapshot_id, path, sha256, bytes) SELECT s, p, h, b::bigint FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[]) AS t(s, p, h, b) ON CONFLICT DO NOTHING",
        staged.files.iter().map(|((snapshot, path), (sha, bytes))| {
            [
                Some(clean(snapshot)),
                Some(clean(path)),
                Some(sha.clone()),
                Some(bytes.to_string()),
            ]
        })
        .collect(),
    )
    .await?;
    insert_rows(
        tx,
        &keyed("evidence", "evidence_id"),
        staged
            .evidence
            .values()
            .map(|e: &Evidence| Ok([Some(e.id.clone()), Some(json(e)?)]))
            .collect::<Result<Vec<_>, StoreError>>()?,
    )
    .await?;
    write_claims(tx, staged.claims.values()).await
}

/// Insert staged claims, then require every durable body to be ours.
async fn write_claims<'a>(
    tx: &Transaction<'_>,
    staged: impl Iterator<Item = &'a Claim>,
) -> Result<(), StoreError> {
    let claims = staged
        .map(|c| Ok((c.id.clone(), json(c)?)))
        .collect::<Result<Vec<_>, StoreError>>()?;
    insert_rows(
        tx,
        "INSERT INTO claims (claim_id, body) SELECT * FROM UNNEST($1::text[], $2::text[]) ON CONFLICT DO NOTHING",
        claims
            .iter()
            .map(|(id, body)| [Some(id.clone()), Some(body.clone())])
            .collect(),
    )
    .await?;
    // A concurrent rival may have committed the same claim id first: the
    // insert above waited for it and kept the winner. Never publish over it.
    let ids: Vec<&str> = claims.iter().map(|(id, _)| id.as_str()).collect();
    let stored: std::collections::BTreeMap<String, String> = tx
        .query(
            "SELECT claim_id, body FROM claims WHERE claim_id = ANY($1)",
            &[&ids],
        )
        .await
        .map_err(pg_error)?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    if claims.iter().any(|(id, body)| stored.get(id) != Some(body)) {
        return Err(StoreError::Invariant(
            "claim identity reused with different or unsealed provenance".into(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl Store for PgStore {
    async fn ensure_snapshot_files(
        &mut self,
        snapshot_id: &str,
        repo: &str,
        files: &[SnapshotFile],
    ) -> Result<(), StoreError> {
        self.staging
            .ensure_snapshot_files(snapshot_id, repo, files)
            .await
    }

    async fn ensure_run(
        &mut self,
        run_id: &str,
        repo: &str,
        snapshot_id: &str,
        set_id: &str,
        catalog_digest: &str,
        rubric_version: &str,
    ) -> Result<(), StoreError> {
        self.staging
            .ensure_run(
                run_id,
                repo,
                snapshot_id,
                set_id,
                catalog_digest,
                rubric_version,
            )
            .await
    }

    async fn put_candidate(&mut self, set_id: &str, c: &Candidate) -> Result<(), StoreError> {
        self.staging.put_candidate(set_id, c).await
    }

    async fn put_entity(&mut self, e: Entity) -> Result<(), StoreError> {
        self.staging.put_entity(e).await
    }

    async fn put_relation(&mut self, r: Relation, build_id: &str) -> Result<(), StoreError> {
        self.staging.put_relation(r, build_id).await
    }

    async fn put_decision(&mut self, d: Decision) -> Result<(), StoreError> {
        for v in d.confidence.iter().chain(&d.probability) {
            finite(*v)?;
        }
        if let Some(raw) = &d.raw_answer {
            validate_raw_finite(raw)?;
        }
        // Durable-first: insert when absent, replace when the cache key
        // changed or a recorded failure is retried, keep otherwise. Then
        // stage the durable winner, never the submitted row.
        let key = decision_key(&d.candidate_id, &d.question_id);
        let failed = matches!(d.outcome, DecisionOutcome::Failed(_));
        let body = json(&d)?;
        self.client()
            .await?
            .execute(
                "INSERT INTO decisions (decision_key, cache_key, failed, body) VALUES ($1, $2, $3, $4) ON CONFLICT (decision_key) DO UPDATE SET cache_key = EXCLUDED.cache_key, failed = EXCLUDED.failed, body = EXCLUDED.body WHERE decisions.cache_key <> EXCLUDED.cache_key OR decisions.failed",
                &[&key, &d.cache_key, &failed, &body],
            )
            .await
            .map_err(pg_error)?;
        let winner = self
            .read_decision(&key)
            .await?
            .ok_or_else(|| StoreError::Invariant("decision missing after write".into()))?;
        self.staging.put_decision(winner).await
    }

    async fn put_inference(&mut self, rec: InferenceRecord) -> Result<(), StoreError> {
        validate_raw_finite(&rec.raw)?;
        // First write wins on the server; stage the winner every racer sees.
        let body = json(&rec)?;
        self.client()
            .await?
            .execute(
                "INSERT INTO inferences (reuse_key, body) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&rec.reuse_key, &body],
            )
            .await
            .map_err(pg_error)?;
        let winner = self
            .read_inference(&rec.reuse_key)
            .await?
            .ok_or_else(|| StoreError::Invariant("inference missing after write".into()))?;
        self.staging.put_inference(winner).await
    }

    async fn find_inference(&self, reuse_key: &str) -> Result<Option<InferenceRecord>, StoreError> {
        // Authoritative first: staging may hold a loser from a lost race.
        if let Some(client) = self.client.as_ref() {
            if let Some(row) = client
                .query_opt(
                    "SELECT body FROM inferences WHERE reuse_key = $1",
                    &[&reuse_key],
                )
                .await
                .map_err(pg_error)?
            {
                return decode(&row.get::<_, String>(0), "inference").map(Some);
            }
        }
        self.staging.find_inference(reuse_key).await
    }

    async fn put_evidence(&mut self, e: Evidence) -> Result<(), StoreError> {
        // Evidence becomes durable inside the publication transaction; the
        // file linkage is still validated before anything is staged.
        if !self.staging.has_file(&e.snapshot, &e.source_file_version) {
            return Err(StoreError::Invariant(format!(
                "evidence {} references unregistered file {} in snapshot {}",
                e.id, e.source_file_version, e.snapshot
            )));
        }
        self.staging.put_evidence(e).await
    }

    async fn put_claim(&mut self, c: Claim) -> Result<(), StoreError> {
        let body = json(&c)?;
        let stored = self
            .client()
            .await?
            .query_opt("SELECT body FROM claims WHERE claim_id = $1", &[&c.id])
            .await
            .map_err(pg_error)?;
        if stored.is_some_and(|r| r.get::<_, String>(0) != body) {
            return Err(StoreError::Invariant(
                "claim identity reused with different or unsealed provenance".into(),
            ));
        }
        self.staging.put_claim(c).await
    }

    async fn find_decision(
        &self,
        candidate_id: &str,
        question_id: &str,
    ) -> Result<Option<Decision>, StoreError> {
        if let Some(client) = self.client.as_ref() {
            let key = decision_key(candidate_id, question_id);
            if let Some(row) = client
                .query_opt(
                    "SELECT body FROM decisions WHERE decision_key = $1",
                    &[&key],
                )
                .await
                .map_err(pg_error)?
            {
                return decode(&row.get::<_, String>(0), "decision").map(Some);
            }
        }
        self.staging.find_decision(candidate_id, question_id).await
    }

    async fn publish(
        &mut self,
        build: GraphBuild,
        expected_predecessor: Option<String>,
    ) -> Result<(), StoreError> {
        let digest = self.staging.publication_digest(&build)?;
        self.publish_tx(build, expected_predecessor, &digest).await
    }

    fn active(&self, repo: &str) -> Option<GraphBuild> {
        self.staging.active(repo)
    }

    fn get(&self, build_id: &str) -> Option<GraphBuild> {
        self.staging.get(build_id)
    }

    fn stats(&self) -> StoreStats {
        self.staging.stats()
    }
}

fn finite(v: f64) -> Result<(), StoreError> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(StoreError::Invariant(format!("non-finite value {v}")))
    }
}

/// Reject non-finite raw floats at the boundary: PostgreSQL would accept
/// NaN/Infinity and JSON would silently encode them as `null`.
fn validate_raw_finite(raw: &RawAnswer) -> Result<(), StoreError> {
    let mut vals: Vec<f64> = Vec::new();
    match raw {
        RawAnswer::Noul { noul } => vals.push(*noul),
        RawAnswer::Choice {
            probabilities,
            confidence,
            ..
        } => {
            vals.push(*confidence);
            vals.extend(probabilities.values().copied());
        }
        RawAnswer::Score {
            score,
            probabilities,
            confidence,
            results,
            ..
        } => {
            vals.push(*score);
            vals.push(*confidence);
            vals.extend(probabilities.values().copied());
            vals.extend(results.iter().copied());
        }
    }
    vals.into_iter().try_for_each(finite)
}
