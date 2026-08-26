//! REST server mode for the Barq memory engine.
//!
//! Thin transport over the exact same `MemoryEngine` the bindings use:
//! embedded and server deployments differ only in configuration, never
//! in behavior.

pub mod api;
pub mod dto;
pub mod state;

pub use api::{router, serve};
pub use state::ServerState;
