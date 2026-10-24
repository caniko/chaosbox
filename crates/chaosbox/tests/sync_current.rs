//! A live request observes one complete view; database faults retain the last good view.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use chaosbox_store::{MemoryReplicaStore, ReplicaStore, ReplicaRow, StoreError};
use chaosbox::sync::{Identity, Replica, current::CurrentReader};
use serde_json::json;

#[derive(Clone, Default)]
struct Shared {
    store: Arc<tokio::sync::Mutex<MemoryReplicaStore>>,
    unavailable: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl ReplicaStore for Shared {
    async fn replica_put(&mut self, row: &ReplicaRow) -> Result<(), StoreError> {
        self.store.lock().await.replica_put(row).await
    }
    async fn replica_rows(
        &mut self,
        user: &str,
        scope: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<ReplicaRow>, StoreError> {
        self.store
            .lock()
            .await
            .replica_rows(user, scope, after, limit)
            .await
    }
    async fn replica_current(
        &mut self,
        user: &str,
        scope: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Connection("test outage".into()));
        }
        self.store.lock().await.replica_current(user, scope).await
    }
    async fn replica_publish(
        &mut self,
        user: &str,
        scope: &str,
        id: &str,
        body: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError> {
        self.store
            .lock()
            .await
            .replica_publish(user, scope, id, body, expected)
            .await
    }
}

#[tokio::test]
async fn live_reader_keeps_last_good_and_refuses_unvalidated_current_views() {
    let (identity, _) = Identity::create("private:can").unwrap();
    let mut shared = Shared::default();
    let view = Replica::new(&identity.user(), "private:can")
        .view()
        .unwrap();
    shared
        .replica_publish(
            &identity.user(),
            "private:can",
            &view.digest,
            &serde_json::to_string(&view).unwrap(),
            None,
        )
        .await
        .unwrap();
    let mut reader = CurrentReader::new(Box::new(shared.clone()), &identity.user(), "private:can");
    let args = json!({"repo":"canix","query":"sync evidence"});
    let good = reader.query("intelligence_context", &args).await.unwrap();
    assert_eq!(good["degraded"], false);
    shared.unavailable.store(true, Ordering::SeqCst);
    let fallback = reader.query("intelligence_context", &args).await.unwrap();
    assert_eq!(fallback["snapshot"], good["snapshot"]);
    assert_eq!(fallback["degraded"], true);
    let mut cold = CurrentReader::new(Box::new(shared.clone()), &identity.user(), "private:can");
    assert!(cold.query("intelligence_context", &args).await.is_err());
    shared.unavailable.store(false, Ordering::SeqCst);
    shared
        .replica_publish(
            &identity.user(),
            "private:can",
            &"a".repeat(64),
            "corrupted snapshot",
            Some(&view.digest),
        )
        .await
        .unwrap();
    let preserved = reader.query("intelligence_context", &args).await.unwrap();
    assert_eq!(preserved["snapshot"], good["snapshot"]);
    assert_eq!(preserved["degraded"], true);
    assert!(reader
        .query(
            "intelligence_context",
            &json!({"repo":"canix","query":"sync","scope":"private:other"})
        )
        .await
        .is_err());
}
