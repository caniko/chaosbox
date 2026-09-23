//! Read-only `chaosbox sessions` surfaces: campaign status and receipt
//! verification. Nothing here writes to a campaign or to a destination
//! database; mutating commands belong to separate, explicitly gated modules.

pub mod campaign;
pub mod cli;
pub mod digest;
pub mod verify;
