//! The read-only `query` command surface (search, lookup, neighbors, path, export, explain, status).

use super::{QueryCmd, AnyReader, consumer_err, EXPORT_NODE_CAP, EXPORT_EDGE_CAP};

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
pub(super) async fn run_query(q: QueryCmd) -> i32 {
    match q {
        QueryCmd::Context {
            query,
            repo,
            depth,
            max_nodes,
            max_chars,
        } => {
            run_navigation(async {
                AnyReader::connect(&repo)
                    .await?
                    .context(&query, depth, max_nodes, max_chars)
                    .await
            })
            .await
        }
        QueryCmd::Stats { repo, limit } => {
            run_navigation(async { AnyReader::connect(&repo).await?.stats(limit).await }).await
        }
        QueryCmd::Community { id, repo, limit } => {
            run_navigation(async { AnyReader::connect(&repo).await?.community(&id, limit).await })
                .await
        }
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
        QueryCmd::Lookup { id, repo, build_id } => {
            let reader = match Box::pin(AnyReader::connect_at(&repo, build_id.as_deref())).await {
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
        QueryCmd::Neighbors {
            id,
            repo,
            rel,
            rel_types,
            limit,
            cursor,
            build_id,
            direction,
        } => {
            let query = chaosbox::NeighborhoodQuery {
                id,
                repo,
                direction,
                limit,
                cursor,
                build_id,
                rel_types: rel_types.or_else(|| rel.map(|name| vec![name])),
            };
            let query = match query.prepare() {
                Ok(query) => query,
                Err(error) => return consumer_err("query neighbors", error),
            };
            match Box::pin(AnyReader::neighborhood_page(query)).await {
                Ok(page) => {
                    println!("{}", serde_json::to_string(&page).unwrap());
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
            let query = chaosbox::PathQuery {
                repo,
                from,
                to,
                max_hops,
            };
            let query = match query.prepare() {
                Ok(query) => query,
                Err(error) => return consumer_err("query path", error),
            };
            match Box::pin(AnyReader::path_query(query)).await {
                // Successful negative (no path) is a null result, exit 0.
                Ok(value) => {
                    println!("{value}");
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
        QueryCmd::Explain { id, repo, build_id } => {
            let reader = match Box::pin(AnyReader::connect_at(&repo, build_id.as_deref())).await {
                Ok(r) => r,
                Err(e) => return consumer_err("query explain", e),
            };
            match reader.explain(&id).await {
                Ok(value) => {
                    println!("{value}");
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
                    "coverage": reader.coverage.as_ref().map(chaosbox_core::coverage::BuildCoverage::report),
                    "export_caps": {"nodes": EXPORT_NODE_CAP, "edges": EXPORT_EDGE_CAP},
                }))
                .unwrap()
            );
            0
        }
    }
}

async fn run_navigation(
    query: impl std::future::Future<Output = Result<serde_json::Value, chaosbox::PipelineError>>,
) -> i32 {
    match query.await {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(error) => consumer_err("query navigation", error),
    }
}
