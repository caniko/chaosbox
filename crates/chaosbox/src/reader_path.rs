//! Prepared ordinary path requests and aggregate-bounded, build-pinned search.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use chaosbox_store::{GraphQueries, NeighborhoodDirection};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use super::{GraphReader, PipelineError, all_relation_types};
use crate::read_view::bounded_json;

const PAGE_SIZE: usize = 200;

/// Trusted caller's work budget for one path search. Prepared requests include
/// acquisition and pinning; existing-reader methods start already pinned. This
/// is not an authorization grant, wire-byte limit or whole-server resource cap.
#[derive(Clone, Copy, Debug)]
pub struct PathBudget {
    /// Distinct visited entities, including the starting entity.
    pub max_visits: usize,
    /// Projected relationship rows, including repeats, self-loops and sentinels.
    pub max_rows: usize,
    /// Logical operations, including acquisition/pinning for prepared requests,
    /// endpoint lookups and empty/sentinel pages. Not driver RPC or scan counts.
    pub max_queries: usize,
    /// Aggregate serialized header/endpoint/page JSON, after bounded decoding.
    pub max_bytes: usize,
    /// Whole search deadline, including acquisition/pinning for prepared requests.
    pub timeout: Duration,
}

impl Default for PathBudget {
    fn default() -> Self {
        Self {
            max_visits: 100_000,
            max_rows: 20_000,
            max_queries: 500,
            max_bytes: 8 * 1024 * 1024,
            timeout: Duration::from_secs(10),
        }
    }
}

/// Ordinary repository path arguments; not a scoped-view authorization grant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathQuery {
    /// Explicit repository label, 1..256 bytes, without control characters.
    pub repo: String,
    /// Starting entity id, 1..256 bytes, without control characters.
    pub from: String,
    /// Target entity id, 1..256 bytes, without control characters.
    pub to: String,
    /// Maximum edges, in 0..=8. Zero permits only an existing self endpoint.
    #[serde(default = "PathQuery::default_hops")]
    pub max_hops: usize,
}

impl PathQuery {
    /// Shared CLI/MCP default hop bound.
    pub const DEFAULT_MAX_HOPS: usize = 4;
    /// Shared CLI/MCP maximum hop bound. Trusted existing-reader calls are separate.
    pub const MAX_HOPS: usize = 8;

    fn default_hops() -> usize {
        Self::DEFAULT_MAX_HOPS
    }

    /// Validate before acquiring a backend or loading credentials. Blank labels
    /// are rejected without trimming or otherwise rewriting entity identities.
    pub fn prepare(self) -> Result<PreparedPath, PipelineError> {
        if ![&self.repo, &self.from, &self.to]
            .iter()
            .all(|id| valid_id(id))
        {
            return Err(PipelineError::Consumer(
                "invalid path repo/endpoint: expected nonblank 1..256 bytes without controls"
                    .into(),
            ));
        }
        if self.max_hops > Self::MAX_HOPS {
            return Err(PipelineError::Consumer(format!(
                "invalid path max_hops: expected 0..={}",
                Self::MAX_HOPS
            )));
        }
        Ok(PreparedPath(self))
    }
}

/// Validated ordinary request, prepared without credentials or a backend handle.
pub struct PreparedPath(PathQuery);

impl PreparedPath {
    /// Acquire, pin and search under the default aggregate budget. The acquisition
    /// future must be lazy: it is first polled after the deadline and work counter
    /// start. The ordinary `{from, to, path}` result preserves null for no path.
    pub async fn read<R: GraphQueries>(
        self,
        acquire: impl std::future::Future<Output = Result<R, PipelineError>>,
    ) -> Result<serde_json::Value, PipelineError> {
        self.read_with_budget(acquire, PathBudget::default()).await
    }

    /// Trusted budget override; budgets are never accepted from tool arguments.
    /// Acquisition and `active_build` each spend one logical operation. The same
    /// counters/deadline then cover endpoints and every directional page.
    pub async fn read_with_budget<R: GraphQueries>(
        self,
        acquire: impl std::future::Future<Output = Result<R, PipelineError>>,
        budget: PathBudget,
    ) -> Result<serde_json::Value, PipelineError> {
        let mut work = Work::new(budget)?;
        before_deadline(work.deadline, async {
            work.start_query().await?;
            let handle = acquire.await?;
            let query = self.0;
            let build = work.query(handle.active_build(&query.repo)).await?;
            work.bytes(&build)?;
            let build = build.ok_or_else(|| {
                PipelineError::Consumer(format!("no active build for repo {}", query.repo))
            })?;
            let reader = GraphReader::from_header(handle, &query.repo, build);
            let path = reader
                .path_inner(&query.from, &query.to, query.max_hops, &mut work)
                .await?;
            Ok(serde_json::json!({"from":query.from, "to":query.to, "path":path}))
        })
        .await
    }
}

struct Work {
    remaining: PathBudget,
    deadline: Instant,
}

fn exhausted(kind: &str) -> PipelineError {
    PipelineError::Consumer(format!(
        "path traversal budget exceeded ({kind}); narrow the query"
    ))
}

fn deadline_error() -> PipelineError {
    PipelineError::Consumer("path traversal deadline exceeded; narrow the query".into())
}

async fn before_deadline<T>(
    deadline: Instant,
    query: impl std::future::Future<Output = Result<T, PipelineError>>,
) -> Result<T, PipelineError> {
    let result = tokio::time::timeout_at(deadline, query)
        .await
        .map_err(|_| deadline_error())?;
    if Instant::now() >= deadline {
        return Err(deadline_error());
    }
    result
}

impl Work {
    fn new(budget: PathBudget) -> Result<Self, PipelineError> {
        Ok(Self {
            remaining: budget,
            deadline: Instant::now()
                .checked_add(budget.timeout)
                .ok_or_else(deadline_error)?,
        })
    }

    async fn start_query(&mut self) -> Result<(), PipelineError> {
        // Yield even when the backend is immediately ready, so timeout and future
        // cancellation remain cooperative for large in-memory traversals too.
        tokio::task::yield_now().await;
        if Instant::now() >= self.deadline {
            return Err(deadline_error());
        }
        self.remaining.max_queries = self
            .remaining
            .max_queries
            .checked_sub(1)
            .ok_or_else(|| exhausted("queries"))?;
        Ok(())
    }

    async fn query<T>(
        &mut self,
        query: impl std::future::Future<Output = Result<T, chaosbox_store::StoreError>>,
    ) -> Result<T, PipelineError> {
        self.start_query().await?;
        query
            .await
            .map_err(|error| PipelineError::Consumer(error.to_string()))
    }

    fn bytes(&mut self, rows: &impl Serialize) -> Result<(), PipelineError> {
        let bytes = bounded_json(rows, self.remaining.max_bytes).map_err(|_| exhausted("bytes"))?;
        self.remaining.max_bytes -= bytes.len();
        Ok(())
    }
}

impl<R: GraphQueries> GraphReader<R> {
    /// Legacy default visit ceiling; rows, queries, bytes and time are also bounded.
    pub const PATH_VISITED_CAP: usize = 100_000;

    /// Shortest undirected path in this pinned build, within `max_hops`.
    /// `None` means complete negative search within that hop bound; exhausted work
    /// is an error. Dropping this future drops the in-flight read, with no worker
    /// spawned. No whole neighborhood or graph export is loaded.
    pub async fn path(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        self.path_with_budget(from, to, max_hops, PathBudget::default())
            .await
    }

    /// [`GraphReader::path`] with a caller-selected visit ceiling and default
    /// row/query/byte/deadline bounds. Kept for existing library callers.
    pub async fn path_with_cap(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
        visit_cap: usize,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        self.path_with_budget(
            from,
            to,
            max_hops,
            PathBudget {
                max_visits: visit_cap,
                ..PathBudget::default()
            },
        )
        .await
    }

    /// Bound the whole already-pinned traversal. A backend must honor the page
    /// contract; invalid/over-returned pages fail instead of producing false
    /// negative reachability. JSON accounting happens after each bounded decode.
    pub async fn path_with_budget(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
        budget: PathBudget,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        if ![from, to].iter().all(|id| valid_id(id)) {
            return Err(PipelineError::Consumer(
                "invalid path endpoint: expected 1..256 bytes".into(),
            ));
        }
        let mut work = Work::new(budget)?;
        before_deadline(
            work.deadline,
            self.path_inner(from, to, max_hops, &mut work),
        )
        .await
    }

    async fn path_inner(
        &self,
        from: &str,
        to: &str,
        max_hops: usize,
        work: &mut Work,
    ) -> Result<Option<Vec<String>>, PipelineError> {
        if work.remaining.max_visits == 0 {
            return Err(exhausted("visits"));
        }
        for id in [from, to] {
            let row = work
                .query(self.handle.entity_by_id(&self.build_id, id))
                .await?;
            work.bytes(&row)?;
            if row.is_none() {
                return Ok(None);
            }
            if from == to {
                return Ok(Some(vec![from.to_owned()]));
            }
        }
        let mut previous = BTreeMap::new();
        let mut seen = BTreeSet::from([from.to_owned()]);
        let mut queue = VecDeque::from([(from.to_owned(), 0)]);
        // ponytail: one node/direction at a time; frontier batching belongs to
        // S04a. Aggregate limits bound this round-trip ceiling in the meantime.
        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            for direction in [
                NeighborhoodDirection::Outgoing,
                NeighborhoodDirection::Incoming,
            ] {
                let mut after: Option<String> = None;
                loop {
                    let limit = PAGE_SIZE.min(work.remaining.max_rows.saturating_add(1));
                    let rows = work
                        .query(self.handle.neighbor_page(
                            &self.build_id,
                            &current,
                            direction,
                            all_relation_types(),
                            after.as_deref(),
                            i64::try_from(limit).expect("page size fits i64"),
                        ))
                        .await?;
                    if rows.len() > limit {
                        return Err(PipelineError::Consumer(
                            "backend path page exceeded limit".into(),
                        ));
                    }
                    work.remaining.max_rows = work
                        .remaining
                        .max_rows
                        .checked_sub(rows.len())
                        .ok_or_else(|| exhausted("rows"))?;
                    work.bytes(&rows)?;
                    for row in &rows {
                        let (anchor, next) = match direction {
                            NeighborhoodDirection::Outgoing => {
                                (&row.from_entity.entity_id, &row.to_entity.entity_id)
                            }
                            NeighborhoodDirection::Incoming => {
                                (&row.to_entity.entity_id, &row.from_entity.entity_id)
                            }
                        };
                        if anchor != &current
                            || !valid_id(next)
                            || after.as_ref().is_some_and(|key| row.rel_id <= *key)
                        {
                            return Err(PipelineError::Consumer(
                                "invalid backend path page".into(),
                            ));
                        }
                        after = Some(row.rel_id.clone());
                        if seen.contains(next) {
                            continue;
                        }
                        if seen.len() >= work.remaining.max_visits {
                            return Err(exhausted("visits"));
                        }
                        seen.insert(next.clone());
                        previous.insert(next.clone(), current.clone());
                        if next == to {
                            let mut path = vec![to.to_owned()];
                            let mut cursor = to;
                            while let Some(parent) = previous.get(cursor) {
                                path.push(parent.clone());
                                cursor = parent;
                            }
                            path.reverse();
                            return Ok(Some(path));
                        }
                        queue.push_back((next.clone(), depth + 1));
                    }
                    if rows.len() < limit {
                        break;
                    }
                }
            }
        }
        Ok(None)
    }
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
