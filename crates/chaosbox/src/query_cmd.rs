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
        QueryCmd::Stats {
            repo,
            limit,
            resolution,
        } => {
            run_navigation(async {
                AnyReader::connect(&repo)
                    .await?
                    .stats_at(limit, resolution)
                    .await
            })
            .await
        }
        QueryCmd::Community {
            id,
            repo,
            limit,
            resolution,
        } => {
            run_navigation(async {
                AnyReader::connect(&repo)
                    .await?
                    .community_at(&id, limit, resolution)
                    .await
            })
            .await
        }
        QueryCmd::Report {
            repo,
            out,
            resolution,
        } => {
            let report = async { AnyReader::connect(&repo).await?.report(resolution).await };
            match report.await {
                Ok(markdown) => emit("query report", &markdown, out.as_deref()),
                Err(e) => consumer_err("query report", e),
            }
        }
        QueryCmd::Evidence {
            rel,
            repo,
            build_id,
        } => {
            run_navigation(async {
                AnyReader::connect_at(&repo, build_id.as_deref())
                    .await?
                    .evidence(&rel)
                    .await
            })
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
            let id = match Box::pin(resolve_arg(&repo, build_id.as_deref(), &id)).await {
                Ok(id) => id,
                Err(code) => return code,
            };
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
            let mut query = chaosbox::NeighborhoodQuery {
                id,
                repo,
                direction,
                limit,
                cursor,
                build_id,
                rel_types: rel_types.or_else(|| rel.map(|name| vec![name])),
            };
            // Validate before any backend work; a continuation cursor already
            // pins the exact entity, otherwise a name resolves to one id.
            if let Err(error) = query.clone().prepare() {
                return consumer_err("query neighbors", error);
            }
            if query.cursor.is_none() {
                let resolved = Box::pin(resolve_arg(
                    &query.repo,
                    query.build_id.as_deref(),
                    &query.id,
                ));
                match resolved.await {
                    Ok(id) => query.id = id,
                    Err(code) => return code,
                }
            }
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
            let mut query = chaosbox::PathQuery {
                repo,
                from,
                to,
                max_hops,
            };
            // Validate before any backend work, then resolve name endpoints.
            if let Err(error) = query.clone().prepare() {
                return consumer_err("query path", error);
            }
            for endpoint in [&mut query.from, &mut query.to] {
                match Box::pin(resolve_arg(&query.repo, None, endpoint)).await {
                    Ok(id) => *endpoint = id,
                    Err(code) => return code,
                }
            }
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
        QueryCmd::Export {
            repo,
            format,
            out,
            community,
            query,
            resolution,
        } => {
            let graph = async { AnyReader::connect(&repo).await?.export().await };
            let rendered = match graph.await {
                Ok(graph) => chaosbox::export::render(
                    &graph,
                    format,
                    community.as_deref(),
                    query.as_deref(),
                    resolution,
                    &repo,
                ),
                Err(e) => return consumer_err("query export", e),
            };
            let files = match rendered {
                Ok(files) => files,
                Err(e) => {
                    return consumer_err("query export", chaosbox::PipelineError::Consumer(e))
                }
            };
            if format != chaosbox::export::Format::Wiki {
                return emit("query export", &files[0].1, out.as_deref());
            }
            let Some(dir) = out else {
                return consumer_err(
                    "query export",
                    chaosbox::PipelineError::Consumer("wiki export requires --out DIR".into()),
                );
            };
            let written = std::fs::create_dir_all(&dir)
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    files
                        .iter()
                        .try_for_each(|(name, text)| write_output(&dir.join(name), text.as_bytes()))
                });
            match written {
                Ok(()) => 0,
                Err(e) => consumer_err("query export", chaosbox::PipelineError::Consumer(e)),
            }
        }
        QueryCmd::Explain { id, repo, build_id } => {
            let id = match Box::pin(resolve_arg(&repo, build_id.as_deref(), &id)).await {
                Ok(id) => id,
                Err(code) => return code,
            };
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

/// CLI entity argument. Ids pass through untouched (a missing id stays a
/// `null` result, exit 0); a name must resolve to exactly one entity, else
/// the candidates print with Graphify's exit codes: 3 ambiguous, 4 absent.
async fn resolve_arg(repo: &str, build_id: Option<&str>, value: &str) -> Result<String, i32> {
    if value.starts_with("ent:") {
        return Ok(value.to_owned());
    }
    let resolved = async {
        AnyReader::connect_at(repo, build_id)
            .await?
            .resolve(value)
            .await
    };
    match resolved.await {
        Ok(chaosbox::Resolution::Entity(row)) => Ok(row.entity_id),
        Ok(chaosbox::Resolution::Ambiguous(candidates)) => {
            println!(
                "{}",
                serde_json::json!({"status":"ambiguous","name":value,"candidates":candidates})
            );
            Err(3)
        }
        Ok(chaosbox::Resolution::Unknown(suggestions)) => {
            println!(
                "{}",
                serde_json::json!({"status":"not_found","name":value,"suggestions":suggestions})
            );
            Err(4)
        }
        Err(e) => Err(consumer_err("query resolve", e)),
    }
}

/// Print `text`, or replace `out` atomically with it.
pub(super) fn emit(command: &str, text: &str, out: Option<&std::path::Path>) -> i32 {
    let Some(path) = out else {
        print!("{text}");
        return 0;
    };
    match write_output(path, text.as_bytes()) {
        Ok(()) => 0,
        Err(e) => consumer_err(command, chaosbox::PipelineError::Consumer(e)),
    }
}

/// Replace `path` atomically (temporary file in the same directory, fsync,
/// rename) so a reader never sees a half-written report or export.
pub(super) fn write_output(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
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
