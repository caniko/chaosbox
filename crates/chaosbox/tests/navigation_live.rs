//! Opt-in read-only qualification against one explicitly selected live build.

use chaosbox_store::GraphQueries;
use chaosbox_typedb::{store::TypeDbConfig, reader::TypeDbReader};

#[tokio::test]
#[ignore = "requires explicit TypeDB credentials and CHAOSBOX_NAVIGATION_BUILD; performs reads only"]
async fn live_neighborhood_limits_ordering_and_membership() {
    let build = std::env::var("CHAOSBOX_NAVIGATION_BUILD").unwrap();
    let mut reader = TypeDbReader::new(TypeDbConfig::from_env().unwrap());
    reader.connect().await.unwrap();
    let edge = reader
        .build_relationships(&build, 1)
        .await
        .unwrap()
        .remove(0);
    let id = &edge.from_entity.entity_id;
    let keys = |rows: Vec<chaosbox_store::RelRow>| {
        rows.into_iter()
            .map(|row| {
                let neighbor = if row.from_entity.entity_id == *id {
                    row.to_entity.entity_id
                } else {
                    row.from_entity.entity_id
                };
                (neighbor, row.rel_id)
            })
            .collect::<Vec<_>>()
    };
    let rows = keys(reader.adjacent_relationships(&build, id, 3).await.unwrap());
    assert!(!rows.is_empty() && rows.len() <= 3);
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        rows,
        keys(reader.adjacent_relationships(&build, id, 3).await.unwrap())
    );
    assert!(reader
        .adjacent_relationships(&build, id, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(reader
        .adjacent_relationships("build:missing", id, 3)
        .await
        .unwrap()
        .is_empty());
    let entities = reader
        .search_entities(&build, "%chaosbox%", 3)
        .await
        .unwrap();
    assert!(entities.len() <= 3);
    assert!(entities.windows(2).all(|pair| {
        (&pair[0].qualified_name, &pair[0].entity_id)
            < (&pair[1].qualified_name, &pair[1].entity_id)
    }));
}
