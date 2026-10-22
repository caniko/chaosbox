//! Strict requests, bounded pagination, and source-filtered query execution.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use super::{Operation, ReadError, ScopedReader, bounded_json};

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Status {
        repo: String,
    },
    Search {
        repo: String,
        query: String,
        limit: Option<usize>,
        cursor: Option<String>,
    },
    Lookup {
        repo: String,
        id: String,
    },
    Neighbors {
        repo: String,
        id: String,
        rel: Option<String>,
        limit: Option<usize>,
        cursor: Option<String>,
    },
    Path {
        repo: String,
        from: String,
        to: String,
        max_hops: Option<usize>,
    },
    Explain {
        repo: String,
        id: String,
    },
    Evidence {
        repo: String,
        rel: String,
    },
    Export {
        repo: String,
        kind: ExportKind,
        limit: Option<usize>,
        cursor: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExportKind {
    Nodes,
    Edges,
}

impl Request {
    pub(super) fn parse(name: &str, mut args: Value) -> Result<Self, ReadError> {
        // Check the closed operation vocabulary before parsing operation args.
        serde_json::from_value::<Operation>(json!(name)).map_err(|_| ReadError::Denied)?;
        let fields = args.as_object_mut().ok_or(ReadError::Arguments)?;
        if fields.contains_key("operation") || fields.values().any(Value::is_null) {
            return Err(ReadError::Arguments);
        }
        fields.insert("operation".into(), json!(name));
        let request: Self = serde_json::from_value(args).map_err(|_| ReadError::Arguments)?;
        let mut strings = vec![request.repo()];
        match &request {
            Self::Search { query, .. } => strings.push(query),
            Self::Lookup { id, .. } | Self::Neighbors { id, .. } | Self::Explain { id, .. } => {
                strings.push(id);
            }
            Self::Evidence { rel, .. } => strings.push(rel),
            Self::Path { from, to, .. } => strings.extend([from.as_str(), to.as_str()]),
            Self::Status { .. } | Self::Export { .. } => {}
        }
        if strings
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 4096)
        {
            return Err(ReadError::Arguments);
        }
        Ok(request)
    }

    pub(super) fn repo(&self) -> &str {
        match self {
            Self::Status { repo }
            | Self::Search { repo, .. }
            | Self::Lookup { repo, .. }
            | Self::Neighbors { repo, .. }
            | Self::Path { repo, .. }
            | Self::Explain { repo, .. }
            | Self::Evidence { repo, .. }
            | Self::Export { repo, .. } => repo,
        }
    }

    pub(super) fn operation(&self) -> Operation {
        match self {
            Self::Status { .. } => Operation::Status,
            Self::Search { .. } => Operation::Search,
            Self::Lookup { .. } => Operation::Lookup,
            Self::Neighbors { .. } => Operation::Neighbors,
            Self::Path { .. } => Operation::Path,
            Self::Explain { .. } => Operation::Explain,
            Self::Evidence { .. } => Operation::Evidence,
            Self::Export { .. } => Operation::Export,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    query: String,
    offset: usize,
}

impl ScopedReader {
    pub(super) async fn query(&self, request: Request) -> Result<Value, ReadError> {
        tokio::task::yield_now().await;
        match &request {
            Request::Status {..} => Ok(json!({"status":"pinned", "nodes":self.nodes.len(),
                "edges":self.edges.len(), "operations": self.view.operations, "budgets":self.view.budgets})),
            Request::Search {query, limit, cursor, ..} => {
                let needle = query.to_lowercase();
                let mut rows: Vec<_> = self.nodes.values().filter(|e|
                    e.name.to_lowercase().contains(&needle) || e.qualified_name.to_lowercase().contains(&needle)).collect();
                rows.sort_by(|a, b| (&a.qualified_name, &a.entity_id).cmp(&(&b.qualified_name, &b.entity_id)));
                self.page(&request, &rows, *limit, cursor.as_deref())
            }
            Request::Lookup {id, ..} => Ok(json!(self.nodes.get(id))),
            Request::Explain {id, ..} => Ok(self.nodes.get(id).map_or(Value::Null, |entity| json!({
                "entity":entity,
                "outgoing":self.edges.values().filter(|e| e.from_entity.entity_id == *id).count(),
                "incoming":self.edges.values().filter(|e| e.to_entity.entity_id == *id).count(),
            }))),
            Request::Neighbors {id, rel, limit, cursor, ..} => {
                let filter = crate::validate_rel_filter(rel.clone().map(|r| vec![r]))
                    .map_err(|_| ReadError::Arguments)?;
                let rows: Vec<_> = self.edges.values().filter(|e|
                    (e.from_entity.entity_id == *id || e.to_entity.entity_id == *id)
                    && filter.as_ref().is_none_or(|f| f.contains(&e.rel_type))).collect();
                self.page(&request, &rows, *limit, cursor.as_deref())
            }
            Request::Path {from, to, max_hops, ..} => {
                let hops = max_hops.unwrap_or(4.min(self.view.budgets.max_hops));
                if hops == 0 || hops > self.view.budgets.max_hops { return Err(ReadError::Arguments); }
                self.path(from, to, hops).await
            }
            Request::Evidence {rel, ..} => Ok(json!({"evidence":self.evidence.get(rel).map(Vec::as_slice).unwrap_or_default()})),
            Request::Export {kind, limit, cursor, ..} => match kind {
                ExportKind::Nodes => self.page(&request, &self.nodes.values().collect::<Vec<_>>(), *limit, cursor.as_deref()),
                ExportKind::Edges => self.page(&request, &self.edges.values().collect::<Vec<_>>(), *limit, cursor.as_deref()),
            },
        }
    }

    fn page<T: Serialize>(
        &self,
        request: &Request,
        rows: &[T],
        limit: Option<usize>,
        cursor: Option<&str>,
    ) -> Result<Value, ReadError> {
        let limit = limit.unwrap_or(20.min(self.view.budgets.max_page_size));
        if limit == 0 || limit > self.view.budgets.max_page_size {
            return Err(ReadError::Arguments);
        }
        let mut arguments = serde_json::to_value(request).map_err(|_| ReadError::Arguments)?;
        arguments
            .as_object_mut()
            .ok_or(ReadError::Arguments)?
            .remove("cursor");
        // Full decision identity binds paging even if a connector mistakenly
        // reuses a view id with another source scope, policy, run or budget.
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(bounded_json(&(&self.view, arguments), 128 * 1024)?)
        );
        let offset = match cursor {
            None => 0,
            Some(cursor) => {
                let cursor: Cursor =
                    serde_json::from_str(cursor).map_err(|_| ReadError::Arguments)?;
                if cursor.query != fingerprint || cursor.offset > rows.len() {
                    return Err(ReadError::Arguments);
                }
                cursor.offset
            }
        };
        let end = offset.saturating_add(limit).min(rows.len());
        let next = if end < rows.len() {
            Some(
                serde_json::to_string(&Cursor {
                    query: fingerprint,
                    offset: end,
                })
                .map_err(|_| ReadError::Arguments)?,
            )
        } else {
            None
        };
        Ok(json!({"items":&rows[offset..end], "next_cursor":next}))
    }

    async fn path(&self, from: &str, to: &str, max_hops: usize) -> Result<Value, ReadError> {
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
            return Ok(Value::Null);
        }
        if from == to {
            return Ok(json!([from]));
        }
        let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for edge in self.edges.values() {
            let (a, b) = (
                edge.from_entity.entity_id.as_str(),
                edge.to_entity.entity_id.as_str(),
            );
            adjacency.entry(a).or_default().insert(b);
            adjacency.entry(b).or_default().insert(a);
        }
        let mut seen = BTreeSet::from([from]);
        let mut previous = BTreeMap::new();
        let mut queue = VecDeque::from([(from, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            tokio::task::yield_now().await;
            if depth >= max_hops {
                continue;
            }
            for next in adjacency.get(node).into_iter().flatten() {
                if !seen.insert(*next) {
                    continue;
                }
                if seen.len() > self.view.budgets.max_visits {
                    return Err(ReadError::Budget);
                }
                previous.insert(*next, node);
                if *next == to {
                    let mut result = vec![to];
                    let mut current = to;
                    while let Some(parent) = previous.get(current) {
                        result.push(parent);
                        current = parent;
                    }
                    result.reverse();
                    return Ok(json!(result));
                }
                queue.push_back((next, depth + 1));
            }
        }
        Ok(Value::Null)
    }
}
