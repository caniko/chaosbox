//! Inserted fixture builds must obey the live reader's publication boundary.

use chaosbox_core::GraphBuild;
use chaosbox_store::{GraphQueries, MemoryReader, MemoryStore, Store};

#[tokio::test]
async fn only_published_builds_have_headers_and_historical_publications_survive() {
    let first = GraphBuild::new("conf", vec!["s1".into()], 1);
    let second = GraphBuild::new("conf", vec!["s2".into()], 2);
    let mut reader = MemoryReader::new();
    reader.insert_build(first.clone());
    reader.insert_build(second.clone());
    assert!(reader
        .published_build("conf", &first.id)
        .await
        .unwrap()
        .is_none());
    assert!(reader
        .published_build("conf", &second.id)
        .await
        .unwrap()
        .is_none());

    reader.set_active("foreign", &first.id);
    assert!(reader.active_build("foreign").await.unwrap().is_none());
    assert!(reader
        .published_build("conf", &first.id)
        .await
        .unwrap()
        .is_none());
    reader.set_active("conf", &first.id);
    reader.set_active("conf", &second.id);
    assert_eq!(
        reader.active_build("conf").await.unwrap().unwrap().build_id,
        second.id
    );
    assert_eq!(
        reader
            .published_build("conf", &first.id)
            .await
            .unwrap()
            .unwrap()
            .build_id,
        first.id
    );
    assert!(reader
        .published_build("foreign", &first.id)
        .await
        .unwrap()
        .is_none());

    // Replacing a staged fixture does not silently inherit publication.
    reader.insert_build(first.clone());
    assert!(reader
        .published_build("conf", &first.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn snapshots_of_the_store_retain_both_active_and_historical_publications() {
    let first = GraphBuild::new("conf", vec!["s1".into()], 1);
    let second = GraphBuild::new("conf", vec!["s2".into()], 2);
    let mut store = MemoryStore::new();
    store.publish(first.clone(), None).await.unwrap();
    store
        .publish(second.clone(), Some(first.id.clone()))
        .await
        .unwrap();
    let reader = MemoryReader::from_store(&store);
    assert_eq!(
        reader.active_build("conf").await.unwrap().unwrap().build_id,
        second.id
    );
    assert_eq!(
        reader
            .published_build("conf", &first.id)
            .await
            .unwrap()
            .unwrap()
            .build_id,
        first.id
    );
}
