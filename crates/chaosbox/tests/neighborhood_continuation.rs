//! Stateless, repository-bound pages must survive pointer movement without full reads.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use chaosbox::{GraphReader, NeighborhoodMode, NeighborhoodQuery};
use chaosbox_core::{Entity, EntityKind, GraphBuild, Relation, RelationScope, RelationType, SourceSpan};
use chaosbox_store::{
    BuildRow, EntityRow, EvidenceRow, GraphQueries, MemoryReader, NeighborhoodDirection, RelRow,
    StoreError,
};
use serde_json::{Value, json};

const REPO: &str = "continuation-pages";

struct Fixture {
    old: GraphBuild,
    new: GraphBuild,
    hub: String,
}

fn fixture() -> Fixture {
    fixture_with_degrees(9, 2)
}

fn fixture_with_degrees(outgoing: usize, incoming: usize) -> Fixture {
    let node = |name: &str| {
        Entity::new(
            EntityKind::Symbol,
            REPO,
            "s-pages",
            "f.rs",
            name,
            name,
            SourceSpan::point("f.rs", 1, 1, 0),
        )
    };
    let hub = node("hub");
    let mut old = GraphBuild::new(REPO, vec!["s-pages".into()], 1);
    old.add_node(hub.clone()).unwrap();
    for index in 0..outgoing.max(incoming) {
        let leaf = node(&format!("leaf-{index}"));
        old.add_node(leaf.clone()).unwrap();
        for (kind, from, to) in [
            (RelationType::Calls, &hub.id, &leaf.id),
            (RelationType::References, &hub.id, &leaf.id),
        ]
        .into_iter()
        .filter(|_| index < outgoing)
        {
            old.add_edge(Relation::new(kind, from, to, RelationScope::File, &old.id))
                .unwrap();
        }
        if index < incoming {
            old.add_edge(Relation::new(
                RelationType::Calls,
                &leaf.id,
                &hub.id,
                RelationScope::File,
                &old.id,
            ))
            .unwrap();
        }
    }
    old.add_edge(Relation::new(
        RelationType::Calls,
        &hub.id,
        &hub.id,
        RelationScope::File,
        &old.id,
    ))
    .unwrap();
    let mut new = GraphBuild::new(REPO, vec!["s-pages".into()], 2);
    new.predecessor = Some(old.id.clone());
    new.add_node(hub.clone()).unwrap();
    Fixture {
        old,
        new,
        hub: hub.id,
    }
}

fn query(f: &Fixture) -> NeighborhoodQuery {
    NeighborhoodQuery {
        repo: REPO.into(),
        id: f.hub.clone(),
        direction: NeighborhoodMode::Both,
        rel_types: None,
        limit: 3,
        cursor: None,
        build_id: None,
    }
}

// Record only page bounds; all collection APIs are forbidden by this fixture.
struct PagesOnly {
    reader: MemoryReader,
    first: bool,
    pages: Arc<Mutex<Vec<(NeighborhoodDirection, i64)>>>,
    headers: Arc<AtomicUsize>,
}

fn handle(f: &Fixture, first: bool, missing: bool) -> PagesOnly {
    let mut reader = MemoryReader::new();
    if !missing {
        reader.insert_build(f.old.clone());
    }
    reader.insert_build(f.new.clone());
    if first {
        reader.set_active(REPO, &f.old.id);
    }
    // Resumption must work even when the active pointer is absent.
    PagesOnly {
        reader,
        first,
        pages: Arc::default(),
        headers: Arc::default(),
    }
}

#[async_trait::async_trait]
impl GraphQueries for PagesOnly {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, StoreError> {
        self.headers.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.first,
            "continuation must not consult the active pointer"
        );
        self.reader.active_build(repo).await
    }
    async fn published_build(
        &self,
        repo: &str,
        build: &str,
    ) -> Result<Option<BuildRow>, StoreError> {
        self.headers.fetch_add(1, Ordering::Relaxed);
        assert!(!self.first, "first page must pin active exactly once");
        self.reader.published_build(repo, build).await
    }
    async fn neighbor_page(
        &self,
        build: &str,
        id: &str,
        direction: NeighborhoodDirection,
        kinds: Vec<String>,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<RelRow>, StoreError> {
        self.pages.lock().unwrap().push((direction, limit));
        self.reader
            .neighbor_page(build, id, direction, kinds, after, limit)
            .await
    }
    async fn entity_by_id(&self, _: &str, _: &str) -> Result<Option<EntityRow>, StoreError> {
        panic!("no entity collection needed")
    }
    async fn search_entities(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<Vec<EntityRow>, StoreError> {
        panic!("no search")
    }
    async fn neighbors_out(
        &self,
        _: &str,
        _: &str,
        _: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        panic!("no outgoing collection")
    }
    async fn neighbors_in(
        &self,
        _: &str,
        _: &str,
        _: Vec<String>,
    ) -> Result<Vec<RelRow>, StoreError> {
        panic!("no incoming collection")
    }
    async fn build_entities(&self, _: &str, _: i64) -> Result<Vec<EntityRow>, StoreError> {
        panic!("no export")
    }
    async fn build_relationships(&self, _: &str, _: i64) -> Result<Vec<RelRow>, StoreError> {
        panic!("no export")
    }
    async fn evidence_for(&self, _: &str, _: &str) -> Result<Vec<EvidenceRow>, StoreError> {
        panic!("no evidence collection")
    }
    async fn evidence_for_limited(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        panic!("no evidence collection")
    }
}

#[tokio::test]
async fn fresh_handles_resume_exact_pages_without_active_pointer_or_collections() {
    let f = fixture();
    let mut request = query(&f);
    let mut outgoing = Vec::new();
    let mut incoming = Vec::new();
    let mut first = true;
    loop {
        let prepared = request.clone().prepare().unwrap();
        let mut backend = handle(&f, first, false);
        if !first && outgoing.len() % 2 == 0 {
            backend.reader.set_active(REPO, &f.new.id);
        }
        let calls = Arc::clone(&backend.pages);
        let page = prepared.read(backend).await.unwrap();
        assert_eq!(page.build_id, f.old.id);
        assert_eq!(page.generation, 1);
        assert!(page.outgoing.len() <= request.limit && page.incoming.len() <= request.limit);
        assert!(calls.lock().unwrap().iter().all(|(_, limit)| *limit == 4));
        if !first {
            assert!(calls
                .lock()
                .unwrap()
                .iter()
                .all(|(dir, _)| *dir == NeighborhoodDirection::Outgoing));
        }
        outgoing.extend(page.outgoing);
        incoming.extend(page.incoming);
        assert_eq!(page.complete, page.next_cursor.is_none());
        assert!(page.incoming_complete);
        if page.complete {
            break;
        }
        request.cursor = page.next_cursor;
        first = false;
        assert!(
            outgoing.len() <= f.old.edges.len(),
            "continuation repeated rows"
        );
    }
    for (actual, direction) in [
        (outgoing, NeighborhoodDirection::Outgoing),
        (incoming, NeighborhoodDirection::Incoming),
    ] {
        let expected = f.old.edges.values().filter(|e| match direction { NeighborhoodDirection::Outgoing => e.from == f.hub, NeighborhoodDirection::Incoming => e.to == f.hub })
            .map(|e| json!({"rel_id":e.id,"rel_type":chaosbox_core::relation_type_name(&e.rel_type),"from_entity":{"entity_id":e.from},"to_entity":{"entity_id":e.to}})).collect::<Vec<_>>();
        assert_eq!(serde_json::to_value(actual).unwrap(), json!(expected));
    }
}

#[tokio::test]
async fn canonical_filters_resume_and_scope_changes_fail_before_backend_reads() {
    let f = fixture();
    let mut request = query(&f);
    request.rel_types = Some(vec!["CALLS".into(), "calls".into()]);
    let first = GraphReader::neighborhood_page(handle(&f, true, false), request.clone())
        .await
        .unwrap();
    request.cursor = first.next_cursor;
    request.rel_types = Some(vec!["Calls".into()]);
    let page = GraphReader::neighborhood_page(handle(&f, false, false), request.clone())
        .await
        .unwrap();
    assert!(!page.outgoing.is_empty());
    for field in ["repo", "id", "direction", "rel_types", "limit"] {
        let mut wrong = serde_json::to_value(&request).unwrap();
        wrong[field] = match field {
            "repo" => json!("foreign"),
            "id" => json!("ent:foreign"),
            "direction" => json!("outgoing"),
            "rel_types" => json!(["references"]),
            _ => json!(2),
        };
        let wrong = serde_json::from_value(wrong).unwrap();
        let backend = handle(&f, true, false);
        let headers = Arc::clone(&backend.headers);
        let error = GraphReader::neighborhood_page(backend, wrong)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cursor"), "{error}");
        assert_eq!(headers.load(Ordering::Relaxed), 0);
    }
    let mut unfiltered = query(&f);
    unfiltered.cursor = GraphReader::neighborhood_page(handle(&f, true, false), unfiltered.clone())
        .await
        .unwrap()
        .next_cursor;
    let mut all = chaosbox::all_relation_types();
    all.reverse();
    all.push("CALLS".into());
    unfiltered.rel_types = Some(all);
    assert!(
        GraphReader::neighborhood_page(handle(&f, false, false), unfiltered)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn explicit_publications_start_and_resume_without_current_fallback() {
    let f = fixture();
    let mut args = serde_json::to_value(query(&f)).unwrap();
    args["build_id"] = json!(f.old.id);
    let request: NeighborhoodQuery = serde_json::from_value(args.clone()).unwrap();
    let page = request
        .prepare()
        .unwrap()
        .read(handle(&f, false, false))
        .await
        .unwrap();
    assert_eq!(page.build_id, f.old.id);
    let cursor = page.next_cursor.unwrap();
    // Version-1 scopes never contained a selector: both old and new cursors
    // keep publication identity in the cursor's existing top-level field.
    let encoded: Value = serde_json::from_str(&cursor).unwrap();
    assert_eq!(encoded["version"], 1);
    assert!(encoded["scope"].get("build_id").is_none());
    args["cursor"] = json!(cursor);
    for selector in [None, Some(json!(f.old.id))] {
        let mut args = args.clone();
        args.as_object_mut().unwrap().remove("build_id");
        if let Some(selector) = selector {
            args["build_id"] = selector;
        }
        let request: NeighborhoodQuery = serde_json::from_value(args).unwrap();
        assert_eq!(
            request
                .prepare()
                .unwrap()
                .read(handle(&f, false, false))
                .await
                .unwrap()
                .build_id,
            f.old.id
        );
    }
    args["build_id"] = json!(f.new.id);
    let request: NeighborhoodQuery = serde_json::from_value(args).unwrap();
    assert!(request
        .prepare()
        .err()
        .unwrap()
        .to_string()
        .contains("conflicts"));

    let foreign = GraphBuild::new("foreign", vec!["s-other".into()], 1);
    for build in ["build:missing", foreign.id.as_str()] {
        let mut args = serde_json::to_value(query(&f)).unwrap();
        args["build_id"] = json!(build);
        let request: NeighborhoodQuery = serde_json::from_value(args).unwrap();
        let mut backend = handle(&f, false, false);
        backend.reader.insert_build(foreign.clone());
        assert!(request
            .prepare()
            .unwrap()
            .read(backend)
            .await
            .unwrap_err()
            .to_string()
            .contains("published build unavailable"));
    }
    // A returned endpoint can become a fresh anchor in the old publication.
    let leaf = f.old.nodes.values().find(|node| node.id != f.hub).unwrap();
    let mut args = serde_json::to_value(query(&f)).unwrap();
    args["build_id"] = json!(f.old.id);
    args["id"] = json!(leaf.id);
    let request: NeighborhoodQuery = serde_json::from_value(args).unwrap();
    let page = request
        .prepare()
        .unwrap()
        .read(handle(&f, false, false))
        .await
        .unwrap();
    assert_eq!(page.incoming.len(), 2);
    assert_eq!(page.incoming[0].from_entity.entity_id, f.hub);
}

#[tokio::test]
async fn both_positions_advance_and_either_direction_can_finish_first() {
    for (outgoing, incoming) in [(9, 9), (2, 9)] {
        let f = fixture_with_degrees(outgoing, incoming);
        let mut request = query(&f);
        let mut rows = [Vec::new(), Vec::new()];
        let mut first = true;
        loop {
            let page = request
                .clone()
                .prepare()
                .unwrap()
                .read(handle(&f, first, false))
                .await
                .unwrap();
            rows[0].extend(page.outgoing);
            rows[1].extend(page.incoming);
            if page.complete {
                break;
            }
            request.cursor = page.next_cursor;
            first = false;
            assert!(rows.iter().all(|rows| rows.len() <= f.old.edges.len()));
        }
        for (actual, direction) in rows.into_iter().zip([
            NeighborhoodDirection::Outgoing,
            NeighborhoodDirection::Incoming,
        ]) {
            let expected = f.old.edges.values().filter(|edge| match direction {
                NeighborhoodDirection::Outgoing => edge.from == f.hub,
                NeighborhoodDirection::Incoming => edge.to == f.hub,
            }).map(|edge| json!({"rel_id":edge.id,"rel_type":chaosbox_core::relation_type_name(&edge.rel_type),
                "from_entity":{"entity_id":edge.from},"to_entity":{"entity_id":edge.to}})).collect::<Vec<_>>();
            assert_eq!(json!(actual), json!(expected));
        }
    }
}

#[tokio::test]
async fn unavailable_build_and_malformed_cursor_never_fall_back_to_current() {
    let f = fixture();
    let mut request = query(&f);
    request.cursor = GraphReader::neighborhood_page(handle(&f, true, false), request.clone())
        .await
        .unwrap()
        .next_cursor;
    let error = GraphReader::neighborhood_page(handle(&f, false, true), request.clone())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("published build unavailable"),
        "{error}"
    );
    let original: Value = serde_json::from_str(request.cursor.as_deref().unwrap()).unwrap();
    for (field, value) in [
        ("version", json!(99)),
        ("contract", json!("scoped-read-view")),
        ("order", json!("descending")),
        ("unexpected", json!(true)),
    ] {
        let mut wrong = original.clone();
        wrong[field] = value;
        request.cursor = Some(wrong.to_string());
        assert!(
            GraphReader::neighborhood_page(handle(&f, true, false), request.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains("cursor")
        );
    }
    for raw in ["not json".into(), "x".repeat(20_000)] {
        request.cursor = Some(raw);
        assert!(
            GraphReader::neighborhood_page(handle(&f, true, false), request.clone())
                .await
                .is_err()
        );
    }
    for (outgoing, incoming) in [
        (json!("start"), original["incoming"].clone()),
        (json!("done"), json!("done")),
        (json!({"after":"\n"}), original["incoming"].clone()),
    ] {
        let mut wrong = original.clone();
        wrong["outgoing"] = outgoing;
        wrong["incoming"] = incoming;
        request.cursor = Some(wrong.to_string());
        assert!(
            GraphReader::neighborhood_page(handle(&f, true, false), request.clone())
                .await
                .is_err()
        );
    }
    let foreign = GraphBuild::new("foreign", vec!["s-other".into()], 1);
    for build in ["build:missing", foreign.id.as_str()] {
        let mut wrong = original.clone();
        wrong["build_id"] = json!(build);
        request.cursor = Some(wrong.to_string());
        let mut backend = handle(&f, false, false);
        backend.reader.insert_build(foreign.clone());
        assert!(GraphReader::neighborhood_page(backend, request.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("published build unavailable"));
    }
}

#[tokio::test]
async fn directional_empty_and_numeric_boundaries_are_explicit() {
    let f = fixture();
    for direction in [
        NeighborhoodMode::Outgoing,
        NeighborhoodMode::Incoming,
        NeighborhoodMode::Both,
    ] {
        let mut request = query(&f);
        request.direction = direction;
        request.rel_types = Some(vec![]);
        let backend = handle(&f, true, false);
        let calls = Arc::clone(&backend.pages);
        let page = GraphReader::neighborhood_page(backend, request)
            .await
            .unwrap();
        assert!(page.complete && page.outgoing.is_empty() && page.incoming.is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }
    for limit in [0, 201, usize::MAX] {
        let mut request = query(&f);
        request.limit = limit;
        assert!(
            GraphReader::neighborhood_page(handle(&f, false, false), request)
                .await
                .is_err()
        );
    }
    for direction in [NeighborhoodMode::Outgoing, NeighborhoodMode::Incoming] {
        let mut request = query(&f);
        request.direction = direction;
        let page = GraphReader::neighborhood_page(handle(&f, true, false), request.clone())
            .await
            .unwrap();
        if direction == NeighborhoodMode::Outgoing {
            assert!(page.incoming_complete && page.incoming.is_empty() && !page.outgoing_complete);
            request.cursor = page.next_cursor;
            let next = GraphReader::neighborhood_page(handle(&f, false, false), request)
                .await
                .unwrap();
            assert!(
                next.incoming_complete && next.incoming.is_empty() && !next.outgoing.is_empty()
            );
        } else {
            assert!(page.outgoing_complete && page.outgoing.is_empty() && page.complete);
        }
    }
    let mut invalid = query(&f);
    invalid.id = "x".repeat(257);
    assert!(
        GraphReader::neighborhood_page(handle(&f, false, false), invalid)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires an explicit disposable s02b2_* TypeDB database and CHAOSBOX_REQUIRE_TYPEDB=1"]
async fn disposable_fresh_readers_resume_old_publication_after_pointer_moves() {
    use chaosbox_core::SnapshotFile;
    use chaosbox_store::Store;
    use chaosbox_typedb::{
        reader::TypeDbReader,
        store::{TypeDbConfig, TypeDbStore},
    };

    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    assert!(
        config.database.starts_with("s02b2_"),
        "use a fresh disposable s02b2_* database"
    );
    let mut store = TypeDbStore::new(config.clone());
    store.migrate().await.unwrap();
    store
        .ensure_snapshot_files(
            "s-pages",
            REPO,
            &[SnapshotFile {
                snapshot: "s-pages".into(),
                path: "f.rs".into(),
                sha256: "fixture".into(),
                bytes: 1,
            }],
        )
        .await
        .unwrap();
    let f = fixture();
    store.publish(f.old.clone(), None).await.unwrap();
    let connect = || async {
        let mut reader = TypeDbReader::new(config.clone());
        reader.connect().await.unwrap();
        reader
    };
    let mut request = query(&f);
    let mut page = GraphReader::neighborhood_page(connect().await, request.clone())
        .await
        .unwrap();
    store
        .publish(f.new.clone(), Some(f.old.id.clone()))
        .await
        .unwrap();
    let active = connect().await.active_build(REPO).await.unwrap().unwrap();
    assert_eq!(active.build_id, f.new.id);
    let mut outgoing = Vec::new();
    let mut incoming = Vec::new();
    loop {
        assert_eq!(page.build_id, f.old.id);
        assert_eq!(page.generation, 1);
        outgoing.extend(page.outgoing);
        incoming.extend(page.incoming);
        if page.complete {
            assert!(page.next_cursor.is_none());
            break;
        }
        request.cursor = page.next_cursor;
        page = GraphReader::neighborhood_page(connect().await, request.clone())
            .await
            .unwrap();
        assert!(outgoing.len() <= f.old.edges.len());
    }
    let mut reference = MemoryReader::new();
    reference.insert_build(f.old.clone());
    assert_eq!(
        json!(outgoing),
        json!(reference
            .neighbors_out(&f.old.id, &f.hub, chaosbox::all_relation_types())
            .await
            .unwrap())
    );
    assert_eq!(
        json!(incoming),
        json!(reference
            .neighbors_in(&f.old.id, &f.hub, chaosbox::all_relation_types())
            .await
            .unwrap())
    );
    eprintln!(
        "{}",
        json!({"case":"historical-continuation","old":f.old.id,"current":f.new.id,"outgoing":outgoing.len(),"incoming":incoming.len()})
    );
}
