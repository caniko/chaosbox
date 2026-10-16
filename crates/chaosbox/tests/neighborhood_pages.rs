//! Shared backend contract: complete directional keyset paging, filtered before limits.

use chaosbox::all_relation_types;
use chaosbox_core::Relation;
use chaosbox_store::{GraphQueries, MemoryReader, NeighborhoodDirection, RelRow};
use serde_json::json;

#[path = "support/neighborhood.rs"]
mod fixture;
use fixture::{DEGREE, REPO, Fixture, anchored, fixture, projection};

async fn page(
    reader: &impl GraphQueries,
    build: &str,
    id: &str,
    direction: NeighborhoodDirection,
    kinds: Vec<String>,
    after: Option<&str>,
    limit: i64,
) -> Vec<RelRow> {
    reader
        .neighbor_page(build, id, direction, kinds, after, limit)
        .await
        .unwrap()
}

async fn check_enumeration(
    reader: &impl GraphQueries,
    fixture: &Fixture,
    direction: NeighborhoodDirection,
    all: &[&Relation],
) -> (usize, usize, usize) {
    let build = &fixture.builds[1];
    let mut page_calls = 0;
    let mut rows_read = 0;
    let mut bytes_read = 0;
    for (kinds, size) in [
        (all_relation_types(), 64_usize),
        (vec!["calls".into(), "calls".into()], 7),
        (vec!["references".into()], 1),
        (vec!["defines".into()], 1),
    ] {
        let expected = all
            .iter()
            .filter(|edge| kinds.contains(&chaosbox_core::relation_type_name(&edge.rel_type)))
            .map(|edge| projection(edge))
            .collect::<Vec<_>>();
        if kinds == ["references"] {
            assert_eq!(expected.len(), 3, "sparse filter must span multiple pages");
        }
        let mut actual = Vec::new();
        let mut after = None;
        let mut delivered_pages = 0;
        loop {
            let mut rows = page(
                reader,
                &build.id,
                &fixture.hub,
                direction,
                kinds.clone(),
                after.as_deref(),
                i64::try_from(size + 1).unwrap(),
            )
            .await;
            assert!(rows.len() <= size + 1);
            assert!(rows.windows(2).all(|pair| pair[0].rel_id < pair[1].rel_id));
            if let Some(after) = &after {
                assert!(rows.iter().all(|row| row.rel_id > *after));
            }
            page_calls += 1;
            rows_read += rows.len();
            bytes_read += serde_json::to_vec(&rows).unwrap().len();
            let more = rows.len() > size;
            rows.truncate(size);
            delivered_pages += 1;
            if let Some(last) = rows.last() {
                after = Some(last.rel_id.clone());
            }
            actual.extend(
                serde_json::to_value(rows)
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned(),
            );
            assert!(actual.len() <= expected.len(), "paging repeated rows");
            if !more {
                break;
            }
        }
        assert_eq!(actual, expected);
        if kinds == ["references"] {
            assert_eq!(
                delivered_pages, 3,
                "resume every sparse page, including the last"
            );
        }
    }
    (page_calls, rows_read, bytes_read)
}

async fn check_empty(
    reader: &impl GraphQueries,
    fixture: &Fixture,
    direction: NeighborhoodDirection,
) {
    let build = &fixture.builds[1];
    for limit in [-1, 0] {
        assert!(page(
            reader,
            &build.id,
            &fixture.hub,
            direction,
            all_relation_types(),
            None,
            limit
        )
        .await
        .is_empty());
    }
    for (selected_build, id, kinds) in [
        (
            build.id.as_str(),
            fixture.isolated.as_str(),
            all_relation_types(),
        ),
        (
            build.id.as_str(),
            fixture.foreign.as_str(),
            all_relation_types(),
        ),
        (
            fixture.builds[2].id.as_str(),
            fixture.hub.as_str(),
            all_relation_types(),
        ),
        ("build:missing", fixture.hub.as_str(), all_relation_types()),
        (build.id.as_str(), "ent:missing", all_relation_types()),
        (build.id.as_str(), fixture.hub.as_str(), vec![]),
        (
            build.id.as_str(),
            fixture.hub.as_str(),
            vec!["\"; delete $x isa code-entity;".into()],
        ),
    ] {
        assert!(page(reader, selected_build, id, direction, kinds, None, 2)
            .await
            .is_empty());
    }
}

async fn check_boundaries(
    reader: &impl GraphQueries,
    fixture: &Fixture,
    direction: NeighborhoodDirection,
    all: &[&Relation],
) {
    let build = &fixture.builds[1];
    let expected = all.iter().map(|edge| projection(edge)).collect::<Vec<_>>();
    for size in [
        1_usize,
        2,
        3,
        7,
        64,
        all.len() - 1,
        all.len(),
        all.len() + 1,
    ] {
        let rows = page(
            reader,
            &build.id,
            &fixture.hub,
            direction,
            all_relation_types(),
            None,
            i64::try_from(size).unwrap(),
        )
        .await;
        assert_eq!(
            serde_json::to_value(rows).unwrap(),
            json!(&expected[..size.min(expected.len())])
        );
    }
    for after in [
        all[0].id.as_str(),
        all[all.len() - 1].id.as_str(),
        "rel:0",
        "~",
        "\"; delete $x isa code-entity;",
    ] {
        let expected = all
            .iter()
            .filter(|edge| edge.id.as_str() > after)
            .take(3)
            .map(|edge| projection(edge))
            .collect::<Vec<_>>();
        let rows = page(
            reader,
            &build.id,
            &fixture.hub,
            direction,
            all_relation_types(),
            Some(after),
            3,
        )
        .await;
        assert_eq!(serde_json::to_value(rows).unwrap(), json!(expected));
    }
    let old = &fixture.builds[0];
    let expected = old
        .edges
        .values()
        .filter(|edge| anchored(edge, &fixture.hub, direction))
        .map(projection)
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(
            page(
                reader,
                &old.id,
                &fixture.hub,
                direction,
                all_relation_types(),
                None,
                2
            )
            .await
        )
        .unwrap(),
        json!(expected)
    );
}

async fn check_pages(reader: &impl GraphQueries, fixture: &Fixture) {
    let build = &fixture.builds[1];
    let mut total = (0, 0, 0);
    for direction in [
        NeighborhoodDirection::Outgoing,
        NeighborhoodDirection::Incoming,
    ] {
        let all = build
            .edges
            .values()
            .filter(|edge| anchored(edge, &fixture.hub, direction))
            .collect::<Vec<_>>();
        assert_eq!(all.len(), DEGREE + 5);
        check_boundaries(reader, fixture, direction, &all).await;
        check_empty(reader, fixture, direction).await;
        let observed = check_enumeration(reader, fixture, direction, &all).await;
        total.0 += observed.0;
        total.1 += observed.1;
        total.2 += observed.2;
    }
    eprintln!(
        "{}",
        json!({"case":"complete-enumeration-only", "build":build.id,"page_calls":total.0,
        "rows_including_sentinels":total.1,"projected_json_bytes":total.2,
        "nodes":build.nodes.len(),"edges":build.edges.len()})
    );
}

#[tokio::test]
async fn reference_directional_pages_are_complete_bounded_and_filtered() {
    let fixture = fixture();
    let mut reader = MemoryReader::new();
    for build in &fixture.builds {
        reader.insert_build(build.clone());
        reader.set_active(&build.repo, &build.id);
    }
    check_pages(&reader, &fixture).await;
}

#[tokio::test]
#[ignore = "requires an explicit disposable s02b1_* TypeDB database and CHAOSBOX_REQUIRE_TYPEDB=1"]
async fn disposable_directional_pages_match_the_complete_fixture() {
    use chaosbox_core::SnapshotFile;
    use chaosbox_store::Store;
    use chaosbox_typedb::{
        reader::TypeDbReader,
        store::{TypeDbConfig, TypeDbStore},
    };
    assert_eq!(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref(), Ok("1"));
    let config = TypeDbConfig::from_env().unwrap();
    assert!(
        config.database.starts_with("s02b1_"),
        "use a disposable s02b1_* database"
    );
    let mut store = TypeDbStore::new(config.clone());
    store.migrate().await.unwrap();
    for (snapshot, repo) in [("s-pages", REPO), ("s-other", "pages-other")] {
        store
            .ensure_snapshot_files(
                snapshot,
                repo,
                &[SnapshotFile {
                    snapshot: snapshot.into(),
                    path: "f.rs".into(),
                    sha256: "fixture".into(),
                    bytes: 1,
                }],
            )
            .await
            .unwrap();
    }
    let fixture = fixture();
    for build in &fixture.builds {
        store
            .publish(build.clone(), build.predecessor.clone())
            .await
            .unwrap();
    }
    let mut reader = TypeDbReader::new(config);
    reader.connect().await.unwrap();
    check_pages(&reader, &fixture).await;
}
