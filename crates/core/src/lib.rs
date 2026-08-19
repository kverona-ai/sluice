//! sluice-core — VCS-neutral domain types, the read-side `GitReader` trait and
//! the backend capability declaration (sluice-doc 02 §1 / §2, 05 §2).
//!
//! This crate must never depend on a UI framework or on a concrete git
//! implementation: it is what the GPUI desktop, the mobile FFI layer and the
//! MCP bridge all share.

pub mod agent;
pub mod backend;
pub mod types;

pub use agent::*;
pub use backend::*;
pub use types::*;
