//! Owner/scope-isolated immutable replication rows and guarded read snapshots.
use std::collections::BTreeMap;
use crate::StoreError;

/// An opaque authenticated application event. Backends enforce isolation and identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaRow {
    /// Content-addressed identity supplied by the application validator.
    pub id: String,
    /// Cryptographic user-root identity.
    pub user: String,
    /// Explicit private scope.
    pub scope: String,
    /// Complete bounded signed envelope, including evidence dependencies.
    pub body: String,
}

/// Durable history plus compare-and-swap publication. A row insert is not publication.
#[async_trait::async_trait]
pub trait ReplicaStore: Send + Sync {
    /// Insert idempotently, rejecting an existing identity with different bytes.
    async fn replica_put(&mut self, row: &ReplicaRow) -> Result<(), StoreError>;
    /// Bounded lexicographic pagination, scoped by both owner and visibility.
    async fn replica_rows(
        &mut self,
        user: &str,
        scope: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<ReplicaRow>, StoreError>;
    /// Pin the last complete validated read snapshot atomically.
    async fn replica_current(
        &mut self,
        user: &str,
        scope: &str,
    ) -> Result<Option<(String, String)>, StoreError>;
    /// Publish only if the expected predecessor is still current.
    async fn replica_publish(
        &mut self,
        user: &str,
        scope: &str,
        id: &str,
        body: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError>;
}

/// Memory implementation with the same identity and predecessor invariants.
#[derive(Default)]
pub struct MemoryReplicaStore {
    rows: BTreeMap<String, ReplicaRow>,
    active: BTreeMap<(String, String), (String, String)>,
    views: BTreeMap<String, (String, String, String)>,
}

/// Validate backend capacity even when a caller does not use the sync frontend.
pub fn validate_row(row: &ReplicaRow) -> Result<(), StoreError> {
    if row.id.len() != 64
        || !row.id.bytes().all(|b| b.is_ascii_hexdigit())
        || row.user.len() != 64
        || !row.user.bytes().all(|b| b.is_ascii_hexdigit())
        || !row.scope.starts_with("private:")
        || row.scope.len() <= 8
        || row.body.len() > 4 * 1024 * 1024
        || row.body.is_empty()
    {
        return Err(StoreError::Invariant(
            "invalid bounded replication row".into(),
        ));
    }
    Ok(())
}

/// Validate a bounded immutable publication at the persistence boundary.
pub fn validate_replica_view(
    user: &str,
    scope: &str,
    id: &str,
    body: &str,
) -> Result<(), StoreError> {
    if id.len() != 64
        || !id.bytes().all(|b| b.is_ascii_hexdigit())
        || user.len() != 64
        || !user.bytes().all(|b| b.is_ascii_hexdigit())
        || !scope.starts_with("private:")
        || scope.len() <= 8
        || scope.len() > 256
        || body.is_empty()
        || body.len() > 64 * 1024 * 1024
    {
        return Err(StoreError::Invariant(
            "invalid bounded replication view".into(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl ReplicaStore for MemoryReplicaStore {
    async fn replica_put(&mut self, row: &ReplicaRow) -> Result<(), StoreError> {
        validate_row(row)?;
        if let Some(old) = self.rows.get(&row.id) {
            if old != row {
                return Err(StoreError::Invariant(
                    "replication identity collision".into(),
                ));
            }
        } else {
            self.rows.insert(row.id.clone(), row.clone());
        }
        Ok(())
    }
    async fn replica_rows(
        &mut self,
        user: &str,
        scope: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<ReplicaRow>, StoreError> {
        if !(1..=200).contains(&limit) {
            return Err(StoreError::Invariant("replica page limit 1..200".into()));
        }
        Ok(self
            .rows
            .values()
            .filter(|r| r.user == user && r.scope == scope && r.id.as_str() > after)
            .take(limit)
            .cloned()
            .collect())
    }
    async fn replica_current(
        &mut self,
        user: &str,
        scope: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        Ok(self.active.get(&(user.into(), scope.into())).cloned())
    }
    async fn replica_publish(
        &mut self,
        user: &str,
        scope: &str,
        id: &str,
        body: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError> {
        validate_replica_view(user, scope, id, body)?;
        let snapshot = (user.into(), scope.into(), body.into());
        if let Some(old) = self.views.get(id) {
            if old != &snapshot {
                return Err(StoreError::Invariant(
                    "replica view identity collision".into(),
                ));
            }
        } else {
            self.views.insert(id.into(), snapshot);
        }
        let current = self.active.get(&(user.into(), scope.into()));
        if current.is_some_and(|(old, bytes)| old == id && bytes == body) {
            return Ok(());
        }
        if current.map(|(id, _)| id.as_str()) != expected {
            return Err(StoreError::Invariant("replica predecessor changed".into()));
        }
        self.active
            .insert((user.into(), scope.into()), (id.into(), body.into()));
        Ok(())
    }
}
