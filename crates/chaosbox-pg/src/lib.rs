//! Chaosbox persistence on PostgreSQL with pgvector: the schema asset, the
//! [`Store`](chaosbox_store::Store) implementation and the read-only reader.
//!
//! Publication stages in a [`MemoryStore`](chaosbox_store::MemoryStore) with
//! identical semantics, then writes the build, its members, sealed evidence
//! and the active-pointer swing in one transaction under a per-repository
//! advisory lock. Readers see a build only after that commit, and published
//! rows are never updated. Embeddings are derived data written afterwards.

mod common;
mod knowledge;
pub mod reader;
pub mod store;

pub use common::{text_sha256, PgConfig};

/// Packaged schema asset (`schema.sql`; Nix source filters must keep it).
pub const SCHEMA_SQL: &str = include_str!("../schema.sql");

/// Schema compatibility marker checked by `db check`.
pub const SCHEMA_VERSION: u32 = 1;

/// PostgreSQL major version this schema is tested against.
pub const POSTGRES_PINNED: &str = "18";

/// Stored embedding width; longer model outputs are truncated and renormalized.
pub const EMBED_DIMS: usize = 1024;
