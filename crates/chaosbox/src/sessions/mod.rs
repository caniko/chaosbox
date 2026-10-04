//! Read-only `chaosbox sessions` surfaces: campaign status and receipt
//! verification. Nothing here writes to a campaign or to a destination
//! database; mutating commands belong to separate, explicitly gated modules.
//!
//! The contract these surfaces enforce — the pinned inventory, the receipt
//! schema, and what a passing check is allowed to claim — is written down in
//! `docs/src/session-verification.md`.

pub mod adopt;
pub mod campaign;
pub mod cli;
pub mod digest;
pub mod inventory;
pub mod remap;
pub mod tool;
pub mod verify;
