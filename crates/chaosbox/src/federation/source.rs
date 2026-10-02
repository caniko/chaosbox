//! Owner-local source adapters. No peer credentials, inference or mutation.
use std::path::{Path, PathBuf};
use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};
use serde::{Deserialize, Serialize};
use crate::intelligence::{Bundle, cli::load_bundle};
use super::ErrorCode;

/// One immutable knowledge generation with its original build identity.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Content identity, retained for exact-generation evidence lookup.
    pub id: String,
    /// Original unmodified bundle with full owner-local receipts.
    pub bundle: Bundle,
}
impl Snapshot {
    /// Pin a validated bundle without modifying scope, ids or receipts.
    pub fn new(bundle: Bundle) -> Result<Self, ErrorCode> {
        let id = bundle.digest().map_err(|_| ErrorCode::InvalidResponse)?;
        Ok(Self { id, bundle })
    }
}

/// Read-only source contract. Historical lookup must never silently use latest.
#[async_trait::async_trait]
pub trait KnowledgeSource: Send + Sync {
    /// Read one complete current generation for the pinned owner-local scope.
    async fn current(&self, scope: &str) -> Result<Snapshot, ErrorCode>;
    /// Resolve the exact requested generation, or return unavailable.
    async fn snapshot(&self, scope: &str, id: &str) -> Result<Option<Snapshot>, ErrorCode>;
}

/// Provider-local backend settings. These never come from the wire request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Backend {
    /// Reviewed artifact, optionally with immutable digest-addressed history.
    Bundle {
        /// Current validated bundle path.
        path: PathBuf,
        /// Optional directory containing `<bundle-digest>.json` artifacts.
        #[serde(default)]
        history: Option<PathBuf>,
    },
    /// Private knowledge generations using provider-local `TypeDB` environment.
    Typedb,
}
impl Backend {
    fn artifact(path: &Path) -> Result<Snapshot, ErrorCode> {
        Snapshot::new(load_bundle(path).map_err(|_| ErrorCode::Unavailable)?)
    }
    fn database(scope: &str, id: String, raw: &str) -> Result<Snapshot, ErrorCode> {
        if raw.len() > 64 * 1024 * 1024
            || chaosbox_core::sha256_hex(&["session-knowledge-v1", scope, raw]) != id
        {
            return Err(ErrorCode::InvalidResponse);
        }
        let bundle = serde_json::from_str(raw).map_err(|_| ErrorCode::InvalidResponse)?;
        Ok(Snapshot { id, bundle })
    }
}
#[async_trait::async_trait]
impl KnowledgeSource for Backend {
    async fn current(&self, scope: &str) -> Result<Snapshot, ErrorCode> {
        match self {
            Self::Bundle { path, .. } => {
                let path = path.clone();
                tokio::task::spawn_blocking(move || Self::artifact(&path))
                    .await
                    .map_err(|_| ErrorCode::Unavailable)?
            }
            Self::Typedb => {
                let mut store =
                    TypeDbStore::new(TypeDbConfig::from_env().map_err(|_| ErrorCode::Unavailable)?);
                match store
                    .knowledge(scope)
                    .await
                    .map_err(|_| ErrorCode::Unavailable)?
                {
                    Some((id, raw)) => Self::database(scope, id, &raw),
                    None => Snapshot::new(Bundle::new(scope)),
                }
            }
        }
    }
    async fn snapshot(&self, scope: &str, id: &str) -> Result<Option<Snapshot>, ErrorCode> {
        if !super::hash(id) {
            return Err(ErrorCode::InvalidRequest);
        }
        match self {
            Self::Bundle { history, .. } => {
                let current = self.current(scope).await?;
                if current.id == id {
                    return Ok(Some(current));
                }
                let Some(history) = history else {
                    return Ok(None);
                };
                let path = history.join(format!("{id}.json"));
                if !path.exists() {
                    return Ok(None);
                }
                let snapshot = tokio::task::spawn_blocking(move || Self::artifact(&path))
                    .await
                    .map_err(|_| ErrorCode::Unavailable)??;
                if snapshot.id != id {
                    return Err(ErrorCode::InvalidResponse);
                }
                Ok(Some(snapshot))
            }
            Self::Typedb => {
                let mut store =
                    TypeDbStore::new(TypeDbConfig::from_env().map_err(|_| ErrorCode::Unavailable)?);
                store
                    .knowledge_at(scope, id)
                    .await
                    .map_err(|_| ErrorCode::Unavailable)?
                    .map(|raw| Self::database(scope, id.into(), &raw))
                    .transpose()
            }
        }
    }
}
