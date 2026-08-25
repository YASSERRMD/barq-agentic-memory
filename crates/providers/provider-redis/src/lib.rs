//! Redis working-memory provider for the Barq memory engine.
//!
//! Session state in a Redis hash (`data` + `revision`) with backend
//! TTL enforcement and an atomic Lua compare-and-set so concurrent
//! tool calls cannot lose each other's updates.

pub mod store;

pub use store::RedisWorkingStore;
