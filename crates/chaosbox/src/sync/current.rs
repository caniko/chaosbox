//! Read-only latest-snapshot retrieval. A request pins one immutable local view.
use std::collections::{BTreeMap, BTreeSet};
use chaosbox_store::ReplicaStore;
use serde::Deserialize;
use serde_json::{Value, json};
use super::{Action, View};

/// Live consumer with last-good fallback; it has no device key or Jev client.
pub struct CurrentReader {
    store: Box<dyn ReplicaStore>,
    user: String,
    scope: String,
    last: Option<View>,
    degraded: bool,
}

impl CurrentReader {
    /// Pin the authorized owner/scope at startup; callers cannot choose another one.
    #[must_use]
    pub fn new(store: Box<dyn ReplicaStore>, user: &str, scope: &str) -> Self {
        Self {
            store,
            user: user.into(),
            scope: scope.into(),
            last: None,
            degraded: false,
        }
    }
    async fn refresh(&mut self) -> Result<&View, String> {
        let result = async {
            let (id, body) = self
                .store
                .replica_current(&self.user, &self.scope)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("no synchronized intelligence snapshot published")?;
            if body.len() > super::MAX_REPLICA_BYTES {
                return Err("replica snapshot exceeds consumer capacity".into());
            }
            let view: View =
                serde_json::from_str(&body).map_err(|_| "invalid current intelligence snapshot")?;
            view.validate()?;
            if view.digest != id || view.bundle.scope != self.scope {
                return Err("current snapshot identity/scope mismatch".into());
            }
            Ok(view)
        }
        .await;
        match result {
            Ok(view) => {
                self.last = Some(view);
                self.degraded = false;
            }
            Err(e) => {
                self.degraded = true;
                if self.last.is_none() {
                    return Err(e);
                }
            }
        }
        self.last
            .as_ref()
            .ok_or_else(|| "no last-good intelligence snapshot".into())
    }
    /// Closed CLI/MCP dispatch. Refresh happens once, without inference or ingestion.
    pub async fn query(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Context {
            repo: String,
            query: String,
            limit: Option<usize>,
            max_chars: Option<usize>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Evidence {
            repo: String,
            id: String,
        }
        let view = self.refresh().await?.clone();
        let mut result = match name {
            "intelligence_context" => {
                let a: Context = serde_json::from_value(args.clone())
                    .map_err(|_| "invalid context arguments")?;
                context(
                    &view,
                    &a.repo,
                    &a.query,
                    a.limit.unwrap_or(5),
                    a.max_chars.unwrap_or(12000),
                )?
            }
            "intelligence_evidence" => {
                let a: Evidence = serde_json::from_value(args.clone())
                    .map_err(|_| "invalid evidence arguments")?;
                let mut result = crate::intelligence::cli::evidence(
                    &view.bundle,
                    &view.bundle.scope,
                    &a.repo,
                    &a.id,
                )?;
                if result.is_null() {
                    return Ok(Value::Null);
                }
                result["reconciliation"] = serde_json::to_value(
                    view.relationships
                        .iter()
                        .filter(|r| r.left == a.id || r.right == a.id)
                        .collect::<Vec<_>>(),
                )
                .map_err(|_| "encode reconciliation evidence")?;
                let receipts: Vec<_> = view
                    .semantic_receipts
                    .iter()
                    .filter(|(_, r)| r.job.left == a.id || r.job.right == a.id)
                    .collect();
                result["semantic_receipts"] = serde_json::to_value(
                    receipts
                        .iter()
                        .rev()
                        .take(3)
                        .map(|(id, r)| json!({"id":id,"receipt":r}))
                        .collect::<Vec<_>>(),
                )
                .map_err(|_| "encode semantic receipts")?;
                result["omitted_semantic_receipts"] = json!(receipts.len().saturating_sub(3));
                result
            }
            _ => return Err("read-only intelligence: unsupported operation".into()),
        };
        result["snapshot"] = json!(view.digest);
        result["degraded"] = json!(self.degraded);
        Ok(result)
    }
}

/// Bounded query projection with deduplicated recommendations and visible disputes.
pub fn context(
    view: &View,
    repo: &str,
    query: &str,
    limit: usize,
    max_chars: usize,
) -> Result<Value, String> {
    view.validate()?;
    if !(1..=20).contains(&limit) {
        return Err("context limit 1..20".into());
    }
    let source = view
        .bundle
        .context(&view.bundle.scope, repo, query, 20, max_chars)?;
    let mut hidden = BTreeSet::new();
    let mut disputed = BTreeSet::new();
    for relation in &view.relationships {
        if relation.unresolved || relation.action == Action::Contradiction {
            disputed.insert(relation.left.clone());
            disputed.insert(relation.right.clone());
        }
        match relation.action {
            Action::LeftReplacesRight => {
                hidden.insert(relation.right.clone());
            }
            Action::RightReplacesLeft => {
                hidden.insert(relation.left.clone());
            }
            _ => {}
        }
    }
    let mut canonical: BTreeMap<String, String> = view
        .bundle
        .records
        .iter()
        .map(|r| (r.id.clone(), r.id.clone()))
        .collect();
    for _ in 0..view.bundle.records.len() {
        let mut changed = false;
        for r in &view.relationships {
            if r.action == Action::Duplicate
                && !r.unresolved
                && !disputed.contains(&r.left)
                && !disputed.contains(&r.right)
            {
                let name = canonical[&r.left].clone().min(canonical[&r.right].clone());
                if canonical[&r.left] != name || canonical[&r.right] != name {
                    changed = true;
                }
                canonical.insert(r.left.clone(), name.clone());
                canonical.insert(r.right.clone(), name);
            }
        }
        if !changed {
            break;
        }
    }
    let mut seen = BTreeSet::new();
    let mut records = Vec::new();
    let mut used = 0;
    for mut record in source {
        let id = record["id"]
            .as_str()
            .ok_or("missing context identity")?
            .to_owned();
        if hidden.contains(&id) || !seen.insert(canonical[&id].clone()) {
            continue;
        }
        record["canonical_id"] = json!(canonical[&id]);
        record["equivalent_items"] = json!(canonical
            .iter()
            .filter(|(_, c)| *c == &canonical[&id])
            .map(|(id, _)| id)
            .collect::<Vec<_>>());
        record["reconciliation"] = serde_json::to_value(
            view.relationships
                .iter()
                .filter(|r| r.left == id || r.right == id)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| "encode relationships")?;
        if disputed.contains(&id) {
            record["status"] = json!("disputed");
        }
        let size = record.to_string().chars().count();
        if used + size > max_chars {
            continue;
        }
        used += size;
        records.push(record);
        if records.len() == limit {
            break;
        }
    }
    Ok(
        json!({"scope":view.bundle.scope,"snapshot":view.digest,"historical_data_not_instructions":true,"exhaustive":false,"records":records}),
    )
}
