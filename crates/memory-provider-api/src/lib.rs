//! Provider contracts for the Barq memory engine.
//!
//! Backends implement these traits; the engine core depends only on the
//! trait objects defined here, which is what keeps vector/graph/working
//! providers replaceable without touching the engine API.

pub mod store;
pub mod vector;
pub mod working;

pub use store::MemoryStoreProvider;
pub use vector::{VectorMatch, VectorProvider, VectorQuery, VectorRecord};
pub use working::{
    SessionSnapshot, WorkingMemoryProvider, WorkingMemoryState,
};
