//! Canonical memory model for the Barq agentic memory engine.
//!
//! This crate is deliberately free of storage and network dependencies: it
//! defines the vocabulary (taxonomy, records, scopes, queries, errors,
//! configuration) that the engine boundary is frozen around.

pub mod id;

pub use id::MemoryId;
