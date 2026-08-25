//! Embedded providers for the Barq memory engine.
//!
//! Volatile in-process storage, single-file persistent storage (redb),
//! and TTL-scoped working memory — everything an agent needs to run
//! with zero external infrastructure.

pub mod filter;
pub mod memory;
// pub mod persistent;
// pub mod working;

pub use memory::InMemoryStore;
// pub use persistent::LocalStore;
// pub use working::InProcessWorkingStore;

/// Default table name shared by the embedded store implementations.
pub(crate) const RECORD_TABLE: &str = "records";
