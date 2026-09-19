//! Chaosbox persistence on TypeDB: schema asset, literal encoding, and the
//! driver-backed [`Store`](chaosbox_gel::Store) implementation.
//!
//! The TypeQL schema mirrors `dbschema/default.esdl`: stable Chaosbox ids
//! are `@key` attributes, endpoints/membership/evidence are typed relations
//! with roles, and timestamps are integer epoch millis. Schema validation
//! is not evidence: outcome and evidence-class strings are validated by the
//! application before insert.

pub mod encode;
pub mod store;

/// Packaged TypeQL schema asset (also present as `schema.tql` in the crate
/// package; Nix source filters must keep `*.tql`).
pub const SCHEMA_TQL: &str = include_str!("../schema.tql");

/// Schema compatibility marker checked by `db check`.
pub const SCHEMA_VERSION: u32 = 1;

/// TypeDB server version this schema is tested against.
pub const TYPEDB_PINNED: &str = "3.13.0";

/// Rust driver version this backend is tested against.
pub const DRIVER_PINNED: &str = "3.12.3";
