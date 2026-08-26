//! Shared server state: one engine behind every route.

use memory_core::MemoryEngine;
use std::sync::Arc;

/// Application state shared by all handlers.
pub struct ServerState {
    /// The exact engine embedded bindings use — no server fork.
    pub engine: Arc<MemoryEngine>,
}

impl ServerState {
    /// Wraps an assembled engine.
    pub fn new(engine: MemoryEngine) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }
}
