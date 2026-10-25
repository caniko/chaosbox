//! Path expansion must use bounded, build-pinned pages rather than whole neighborhoods.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use chaosbox::{GraphReader, PathBudget, PathQuery};
use chaosbox_core::{Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SourceSpan};
use chaosbox_store::{
    BuildRow, EntityRow, EvidenceRow, GraphQueries, MemoryReader, NeighborhoodDirection, RelRow,
    StoreError,
};

const REPO: &str = "path-budgets";
const DEGREE: usize = 450;

#[derive(Default)]
struct Observations {
    pages: Mutex<Vec<(NeighborhoodDirection, Option<String>, i64)>>,
    queries: AtomicUsize,
    pins: AtomicUsize,
    pending: AtomicUsize,
    block_pages: AtomicBool,
    bad_page: AtomicUsize,
    page_delay: Mutex<Duration>,
    pin_delay: Mutex<Duration>,
}

struct Pending(Arc<Observations>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

struct PagedOnly {
    inner: MemoryReader,
    observations: Arc<Observations>,
}

#[async_trait::async_trait]
impl GraphQueries for PagedOnly {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError> {
        self.observations.pins.fetch_add(1, Ordering::SeqCst);
        self.observations.pending.fetch_add(1, Ordering::SeqCst);
        let _pending = Pending(self.observations.clone());
        let delay = *self.observations.pin_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        self.inner.active_build(repo).await
    }

    async fn published_build(
        &self,
        repo: &str,
        build: &str,
    ) -> Result<Option<BuildRow>, StoreError> {
        self.inner.published_build(repo, build).await
    }

    async fn entity_by_id(&self, build: &str, id: &str) -> Result<Option<EntityRow>, StoreError> {
        self.observations.queries.fetch_add(1, Ordering::SeqCst);
        self.inner.entity_by_id(build, id).await
    }

    async fn neighbor_page(
        &self,
        build: &str,
        id: &str,
        direction: NeighborhoodDirection,
        types: Vec<String>,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        assert!((1..=200).contains(&limit), "unbounded page: {limit}");
        self.observations.queries.fetch_add(1, Ordering::SeqCst);
        self.observations.pending.fetch_add(1, Ordering::SeqCst);
        let _pending = Pending(self.observations.clone());
        self.observations
            .pages
            .lock()
            .unwrap()
            .push((direction, after.map(str::to_owned), limit));
        if self.observations.block_pages.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let delay = *self.observations.page_delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        let mut rows = self
            .inner
            .neighbor_page(build, id, direction, types, after, limit)
            .await?;
        if let Some(first) = rows.first().cloned() {
            match self.observations.bad_page.load(Ordering::SeqCst) {
                1 => rows.resize(usize::try_from(limit + 1).unwrap(), first),
                2 => rows[0].from_entity.entity_id = "foreign".into(),
                3 => rows[1] = first,
                4 => rows[0].rel_type = "x".repeat(4096),
                _ => (),
            }
        }
        Ok(rows)
    }

    async fn search_entities(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        panic!("path must not search/export the build");
    }

    async fn neighbors_out(
        &self,
        _: &str,
        _: &str,
        _: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        panic!("unbounded outgoing neighborhood");
    }

    async fn neighbors_in(
        &self,
        _: &str,
        _: &str,
        _: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        panic!("unbounded incoming neighborhood");
    }

    async fn build_entities(&self, _: &str, _: i64) -> Result<Vec<EntityRow>, StoreError> {
        panic!("path must not export entities");
    }

    async fn build_relationships(&self, _: &str, _: i64) -> Result<Vec<RelRow>, StoreError> {
        panic!("path must not export relationships");
    }

    async fn evidence_for(&self, _: &str, _: &str) -> Result<Vec<EvidenceRow>, StoreError> {
        panic!("path does not load evidence");
    }

    async fn evidence_for_limited(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        panic!("path does not load evidence");
    }
}

fn handle_fixture() -> (PagedOnly, Arc<Observations>, String, String, String) {
    let node = |name: &str| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            "s-path",
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = node("hub");
    let isolated = node("isolated");
    let mut build = GraphBuild::new(REPO, vec!["s-path".into()], 1);
    build.add_node(hub.clone()).unwrap();
    build.add_node(isolated.clone()).unwrap();
    for index in 0..DEGREE {
        let leaf = node(&format!("leaf-{index}"));
        build.add_node(leaf.clone()).unwrap();
        build
            .add_edge(Relation::new(
                RelationType::Calls,
                &hub.id,
                &leaf.id,
                RelationScope::File,
                &build.id,
            ))
            .unwrap();
    }
    // The final relationship in backend order is deliberately beyond two pages.
    let late = build.edges.last_key_value().unwrap().1.to.clone();
    let mut inner = MemoryReader::new();
    let build_id = build.id.clone();
    inner.insert_build(build);
    inner.set_active(REPO, &build_id);
    let observations = Arc::new(Observations::default());
    (
        PagedOnly {
            inner,
            observations: observations.clone(),
        },
        observations,
        hub.id,
        late,
        isolated.id,
    )
}

async fn fixture() -> (
    GraphReader<PagedOnly>,
    Arc<Observations>,
    String,
    String,
    String,
) {
    let (handle, observations, hub, late, isolated) = handle_fixture();
    let reader = GraphReader::pinned(handle, REPO).await.unwrap();
    (reader, observations, hub, late, isolated)
}

fn request(from: &str, to: &str, max_hops: usize) -> chaosbox::PreparedPath {
    PathQuery {
        repo: REPO.into(),
        from: from.into(),
        to: to.into(),
        max_hops,
    }
    .prepare()
    .unwrap()
}

#[tokio::test]
async fn finds_a_late_page_and_preserves_undirected_traversal() {
    let (reader, observations, hub, late, _) = fixture().await;
    assert_eq!(
        reader.path(&hub, &late, 1).await.unwrap(),
        Some(vec![hub.clone(), late.clone()])
    );
    assert_eq!(observations.pages.lock().unwrap().len(), 3);
    assert_eq!(
        reader.path(&late, &hub, 1).await.unwrap(),
        Some(vec![late, hub])
    );
}

#[tokio::test]
async fn no_path_requires_complete_expansion_and_respects_hop_bound() {
    let (reader, observations, hub, _, isolated) = fixture().await;
    assert_eq!(reader.path(&hub, &isolated, 1).await.unwrap(), None);
    assert_eq!(observations.pages.lock().unwrap().len(), 4);
    assert_eq!(reader.path(&hub, &isolated, 0).await.unwrap(), None);
}

#[tokio::test]
async fn exhausted_rows_queries_visits_and_bytes_are_errors_not_negative_results() {
    let (reader, observations, hub, late, isolated) = fixture().await;
    for (budget, reason, expected_queries) in [
        (
            PathBudget {
                max_rows: 200,
                ..PathBudget::default()
            },
            "rows",
            4,
        ),
        (
            PathBudget {
                max_queries: 3,
                ..PathBudget::default()
            },
            "queries",
            3,
        ),
        (
            PathBudget {
                max_visits: 2,
                ..PathBudget::default()
            },
            "visits",
            3,
        ),
        (
            PathBudget {
                max_bytes: 1,
                ..PathBudget::default()
            },
            "bytes",
            1,
        ),
    ] {
        observations.queries.store(0, Ordering::SeqCst);
        let error = reader
            .path_with_budget(&hub, &isolated, 1, budget)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("budget exceeded ({reason})")),
            "{error}"
        );
        assert_eq!(
            observations.queries.load(Ordering::SeqCst),
            expected_queries
        );
    }
    // A reachable target beyond the row budget must also be an explicit error.
    let budget = PathBudget {
        max_rows: 200,
        ..PathBudget::default()
    };
    assert!(reader
        .path_with_budget(&hub, &late, 1, budget)
        .await
        .is_err());
    assert!(reader.path(&hub, &late, 1).await.unwrap().is_some());
}

#[tokio::test]
async fn exact_row_budget_can_prove_completion_with_empty_pages() {
    let (reader, observations, hub, _, isolated) = fixture().await;
    let budget = PathBudget {
        max_rows: DEGREE,
        ..PathBudget::default()
    };
    assert!(reader
        .path_with_budget(&hub, &isolated, 1, budget)
        .await
        .unwrap()
        .is_none());
    let pages = observations.pages.lock().unwrap();
    assert_eq!(pages.len(), 4); // 200 + 200 + 50, then one empty incoming probe.
    assert_eq!(pages[2].2, 51); // A sentinel beyond the remaining row budget.
    assert_eq!(pages[3].2, 1);
}

#[tokio::test]
async fn missing_self_and_invalid_endpoints_do_not_fabricate_paths() {
    let (reader, observations, hub, _, _) = fixture().await;
    assert_eq!(
        reader.path(&hub, &hub, 0).await.unwrap(),
        Some(vec![hub.clone()])
    );
    assert_eq!(reader.path("missing", "missing", 0).await.unwrap(), None);
    assert_eq!(reader.path(&hub, "missing", 1).await.unwrap(), None);
    assert!(reader.path("", &hub, 1).await.is_err());
    assert!(reader.path(&"x".repeat(257), &hub, 1).await.is_err());
    assert!(observations.pages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn over_return_wrong_anchor_and_nonprogressing_pages_fail_closed() {
    let (reader, observations, hub, _, isolated) = fixture().await;
    for mode in 1..=3 {
        observations.bad_page.store(mode, Ordering::SeqCst);
        let error = reader.path(&hub, &isolated, 1).await.unwrap_err();
        let expected = if mode == 1 {
            "exceeded limit"
        } else {
            "invalid backend"
        };
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[tokio::test]
async fn byte_budget_accounts_for_bounded_pages_not_only_endpoints() {
    let (reader, observations, hub, _, isolated) = fixture().await;
    observations.bad_page.store(4, Ordering::SeqCst);
    let budget = PathBudget {
        max_bytes: 2048,
        ..PathBudget::default()
    };
    let error = reader
        .path_with_budget(&hub, &isolated, 1, budget)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("budget exceeded (bytes)"),
        "{error}"
    );
    assert_eq!(observations.pages.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn whole_search_deadline_drops_a_pending_read() {
    let (reader, observations, hub, _, isolated) = fixture().await;
    observations.block_pages.store(true, Ordering::SeqCst);
    let budget = PathBudget {
        timeout: Duration::from_millis(25),
        ..PathBudget::default()
    };
    let error = reader
        .path_with_budget(&hub, &isolated, 1, budget)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deadline exceeded"), "{error}");
    assert_eq!(observations.pending.load(Ordering::SeqCst), 0);
    assert_eq!(observations.pages.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn deadline_is_not_reset_between_slow_pages() {
    let (reader, observations, hub, _, isolated) = fixture().await;
    *observations.page_delay.lock().unwrap() = Duration::from_millis(15);
    let budget = PathBudget {
        timeout: Duration::from_millis(25),
        ..PathBudget::default()
    };
    let error = reader
        .path_with_budget(&hub, &isolated, 1, budget)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deadline exceeded"), "{error}");
    assert!(observations.pages.lock().unwrap().len() <= 2);
    assert_eq!(observations.pending.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dropping_a_search_drops_its_read_and_later_requests_still_work() {
    let (reader, observations, hub, late, isolated) = fixture().await;
    observations.block_pages.store(true, Ordering::SeqCst);
    let reader = Arc::new(reader);
    let task_reader = reader.clone();
    let task_hub = hub.clone();
    let task = tokio::spawn(async move { task_reader.path(&task_hub, &isolated, 1).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while observations.pending.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(observations.pending.load(Ordering::SeqCst), 0);
    observations.block_pages.store(false, Ordering::SeqCst);
    assert!(reader.path(&hub, &late, 1).await.unwrap().is_some());
}

#[tokio::test]
async fn prepared_request_shares_exact_query_and_row_boundaries_with_acquisition() {
    for maximum in [0, 1, 7, 8] {
        let (handle, observations, hub, _, isolated) = handle_fixture();
        let acquired = AtomicBool::new(false);
        let acquire = async {
            acquired.store(true, Ordering::SeqCst);
            Ok(handle)
        };
        let result = request(&hub, &isolated, 1)
            .read_with_budget(
                acquire,
                PathBudget {
                    max_rows: DEGREE,
                    max_queries: maximum,
                    ..PathBudget::default()
                },
            )
            .await;
        assert_eq!(acquired.load(Ordering::SeqCst), maximum > 0);
        assert_eq!(
            observations.pins.load(Ordering::SeqCst),
            usize::from(maximum > 1)
        );
        if maximum == 8 {
            assert_eq!(result.unwrap()["path"], serde_json::Value::Null);
            assert_eq!(observations.queries.load(Ordering::SeqCst), 6);
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("budget exceeded (queries)"));
        }
    }
}

#[tokio::test]
async fn prepared_byte_budget_includes_the_header_and_allows_the_exact_total() {
    for short_by in [0, 1] {
        let (handle, observations, hub, _, _) = handle_fixture();
        let header = handle.inner.active_build(REPO).await.unwrap();
        let endpoint = handle
            .inner
            .entity_by_id(&header.as_ref().unwrap().build_id, &hub)
            .await
            .unwrap();
        let bytes = serde_json::to_vec(&header).unwrap().len()
            + serde_json::to_vec(&endpoint).unwrap().len();
        let result = request(&hub, &hub, 0)
            .read_with_budget(
                async { Ok(handle) },
                PathBudget {
                    max_visits: 1,
                    max_rows: 0,
                    max_queries: 3,
                    max_bytes: bytes - short_by,
                    ..PathBudget::default()
                },
            )
            .await;
        if short_by == 0 {
            assert_eq!(result.unwrap()["path"], serde_json::json!([hub]));
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("budget exceeded (bytes)"));
        }
        assert_eq!(observations.pins.load(Ordering::SeqCst), 1);
        assert_eq!(observations.queries.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn prepared_deadline_covers_pending_acquisition_and_pinning() {
    for block_acquisition in [true, false] {
        let (handle, observations, hub, _, isolated) = handle_fixture();
        *observations.pin_delay.lock().unwrap() = Duration::from_secs(5);
        let acquire = async {
            observations.pending.fetch_add(1, Ordering::SeqCst);
            let _pending = Pending(observations.clone());
            if block_acquisition {
                std::future::pending::<()>().await;
            }
            Ok(handle)
        };
        let error = request(&hub, &isolated, 1)
            .read_with_budget(
                acquire,
                PathBudget {
                    timeout: Duration::from_millis(25),
                    ..PathBudget::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deadline exceeded"), "{error}");
        assert_eq!(observations.pending.load(Ordering::SeqCst), 0);
        assert_eq!(
            observations.pins.load(Ordering::SeqCst),
            usize::from(!block_acquisition)
        );
        assert_eq!(observations.queries.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn prepared_deadline_is_not_reset_after_acquisition_or_pinning() {
    let (handle, observations, hub, _, isolated) = handle_fixture();
    *observations.pin_delay.lock().unwrap() = Duration::from_millis(40);
    *observations.page_delay.lock().unwrap() = Duration::from_millis(40);
    let acquire = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok(handle)
    };
    let error = request(&hub, &isolated, 1)
        .read_with_budget(
            acquire,
            PathBudget {
                timeout: Duration::from_millis(100),
                ..PathBudget::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deadline exceeded"), "{error}");
    assert!(observations.pages.lock().unwrap().len() <= 1);
    assert_eq!(observations.pending.load(Ordering::SeqCst), 0);
}
