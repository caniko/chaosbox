//! Bounded member reads and immutable publication analytics.

use chaosbox_store::{GraphQueries, conformance_seed};

#[tokio::test]
async fn navigation_queries_stay_with_the_requested_build() {
    let seed = conformance_seed();
    let rows = seed
        .reader
        .adjacent_relationships(&seed.builds.0, &seed.a1, 1)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rel_id, seed.rel1);
    assert!(seed
        .reader
        .adjacent_relationships(&seed.builds.1, &seed.a1, 1)
        .await
        .unwrap()
        .is_empty());
    assert!(seed
        .reader
        .adjacent_relationships(&seed.builds.0, &seed.a1, 0)
        .await
        .unwrap()
        .is_empty());
    let old = seed
        .reader
        .navigation_summary(&seed.builds.0)
        .await
        .unwrap()
        .unwrap();
    let current = seed
        .reader
        .navigation_summary(&seed.builds.1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old["build_id"], seed.builds.0);
    assert_eq!(current["build_id"], seed.builds.1);
    assert_eq!(old["nodes"], 2);
    assert_eq!(old["edges"], 1);
    assert!(seed
        .reader
        .navigation_summary("missing")
        .await
        .unwrap()
        .is_none());
}
