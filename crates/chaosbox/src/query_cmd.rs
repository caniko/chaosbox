//! The read-only `query` command surface (search, lookup, neighbors, path, export, explain, status).

use super::{QueryCmd, AnyReader, consumer_err, EXPORT_NODE_CAP, EXPORT_EDGE_CAP};

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
pub(super) async fn run_query(q: QueryCmd) -> i32 {
    match q {
        QueryCmd::Search { query, repo, limit } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query search", e),
            };
            match reader.search(&query, limit).await {
                Ok(rows) => {
                    println!("{}", serde_json::to_string(&rows).unwrap());
                    0
                }
                Err(e) => consumer_err("query search", e),
            }
        }
        QueryCmd::Lookup { id, repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query lookup", e),
            };
            match reader.lookup(&id).await {
                Ok(row) => {
                    println!("{}", serde_json::to_string(&row).unwrap());
                    0
                }
                Err(e) => consumer_err("query lookup", e),
            }
        }
        QueryCmd::Neighbors { id, repo, rel } => {
            // Validate the filter before touching the backend: typos must fail loudly.
            let filter = match chaosbox::validate_rel_filter(rel.map(|r| vec![r])) {
                Ok(f) => f,
                Err(e) => return consumer_err("query neighbors", e),
            };
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query neighbors", e),
            };
            match reader.neighbors(&id, filter).await {
                Ok((out, inc)) => {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "id": id, "outgoing": out, "incoming": inc,
                        }))
                        .unwrap()
                    );
                    0
                }
                Err(e) => consumer_err("query neighbors", e),
            }
        }
        QueryCmd::Path {
            from,
            to,
            repo,
            max_hops,
        } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query path", e),
            };
            match reader.path(&from, &to, max_hops).await {
                // Successful negative (no path) is a null result, exit 0.
                Ok(path) => {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "from": from, "to": to, "path": path,
                        }))
                        .unwrap()
                    );
                    0
                }
                Err(e) => consumer_err("query path", e),
            }
        }
        QueryCmd::Export { repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query export", e),
            };
            match reader.export().await {
                Ok(v) => {
                    println!("{}", serde_json::to_string(&v).unwrap());
                    0
                }
                Err(e) => consumer_err("query export", e),
            }
        }
        QueryCmd::Explain { id, repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query explain", e),
            };
            match reader.lookup(&id).await {
                Ok(None) => {
                    println!("null");
                    0
                }
                Ok(Some(e)) => {
                    let (out, inc) = match reader.neighbors(&id, None).await {
                        Ok(n) => n,
                        Err(e) => return consumer_err("query explain", e),
                    };
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "id": e.entity_id, "kind": e.kind, "file": e.file,
                            "qualified_name": e.qualified_name,
                            "outgoing": out.len(), "incoming": inc.len(),
                        }))
                        .unwrap()
                    );
                    0
                }
                Err(e) => consumer_err("query explain", e),
            }
        }
        QueryCmd::Status { repo } => {
            let reader = match Box::pin(AnyReader::connect(&repo)).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query status", e),
            };
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "repo": repo, "build_id": reader.build_id(),
                    "generation": reader.generation(),
                    "status": reader.status(),
                    "snapshots": reader.snapshots(),
                    "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
                }))
                .unwrap()
            );
            0
        }
    }
}
