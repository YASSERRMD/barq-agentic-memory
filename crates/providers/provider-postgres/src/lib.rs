//! PostgreSQL canonical store for the Barq memory engine.
//!
//! Authoritative long-term persistence with append-only version
//! history, temporal validity, soft delete, retention metadata, and
//! optimistic concurrency.

pub mod mapping;
pub mod store;

pub use store::PostgresStore;

/// Embedded migration applied on connect.
pub const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_canonical_memories",
    include_str!("../migrations/0001_canonical_memories.sql"),
)];
