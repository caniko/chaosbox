//! Chaosbox persistence on `TypeDB`: schema asset, literal encoding, and the
//! driver-backed [`Store`](chaosbox_store::Store) implementation.
//!
//! The `TypeQL` schema uses `@key` attributes for stable Chaosbox ids,
//! typed relations with roles for endpoints/membership/evidence, and
//! integer epoch millis for timestamps. Schema validation
//! is not evidence: outcome and evidence-class strings are validated by the
//! application before insert.

mod common;
pub mod encode;
pub mod reader;
pub mod store;

/// Packaged `TypeQL` schema asset (also present as `schema.tql` in the crate
/// package; Nix source filters must keep `*.tql`).
pub const SCHEMA_TQL: &str = include_str!("../schema.tql");

/// Schema compatibility marker checked by `db check`.
pub const SCHEMA_VERSION: u32 = 1;

/// `TypeDB` server version this schema is tested against.
pub const TYPEDB_PINNED: &str = "3.13.0";

/// Rust driver version this backend is tested against.
pub const DRIVER_PINNED: &str = "3.12.3";
