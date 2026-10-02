//! Validated event ingestion and last-good read-view publication.
use chaosbox_store::{ReplicaStore, ReplicaRow};
use super::{Replica, SignedEvent, View, MAX_EVENTS, MAX_REPLICA_BYTES};

/// Load every bounded history page; never treat a missing backend as an empty replica.
pub async fn load_replica(
    store: &mut impl ReplicaStore,
    user: &str,
    scope: &str,
) -> Result<Replica, String> {
    let mut replica = Replica::new(user, scope);
    let mut after = String::new();
    let mut bytes = 0usize;
    loop {
        let rows = store
            .replica_rows(user, scope, &after, 200)
            .await
            .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            bytes = bytes
                .checked_add(row.body.len())
                .ok_or("replica byte overflow")?;
            if bytes > MAX_REPLICA_BYTES || replica.events.len() >= MAX_EVENTS {
                return Err("replica exceeds bounded scope capacity".into());
            }
            if row.user != user || row.scope != scope || row.id <= after {
                return Err("backend returned an unscoped or unordered replica page".into());
            }
            let event: SignedEvent = serde_json::from_str(&row.body)
                .map_err(|_| "invalid stored replication envelope")?;
            if event.id != row.id {
                return Err("stored replication identity mismatch".into());
            }
            after = row.id;
            replica.receive(event)?;
        }
    }
    Ok(replica)
}

/// Authenticate the complete envelope before making any durable write.
pub async fn persist_event(
    store: &mut impl ReplicaStore,
    user: &str,
    scope: &str,
    event: &SignedEvent,
) -> Result<(), String> {
    event.validate(user, scope)?;
    store
        .replica_put(&ReplicaRow {
            id: event.id.clone(),
            user: user.into(),
            scope: scope.into(),
            body: serde_json::to_string(event).map_err(|_| "encode replication envelope")?,
        })
        .await
        .map_err(|e| e.to_string())
}

/// Re-read and merge on predecessor contention; incomplete dependencies retain last-good.
pub async fn publish_current(
    store: &mut impl ReplicaStore,
    user: &str,
    scope: &str,
) -> Result<View, String> {
    for _ in 0..3 {
        let previous = store
            .replica_current(user, scope)
            .await
            .map_err(|e| e.to_string())?;
        let view = load_replica(store, user, scope).await?.view()?;
        let bytes = serde_json::to_string(&view).map_err(|_| "encode replica view")?;
        if bytes.len() > MAX_REPLICA_BYTES {
            return Err("replica snapshot exceeds bounded publication capacity".into());
        }
        match store
            .replica_publish(
                user,
                scope,
                &view.digest,
                &bytes,
                previous.as_ref().map(|(id, _)| id.as_str()),
            )
            .await
        {
            Ok(()) => return Ok(view),
            Err(chaosbox_store::StoreError::Invariant(_)) => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("concurrent replica publication; retry after peer exchange".into())
}
