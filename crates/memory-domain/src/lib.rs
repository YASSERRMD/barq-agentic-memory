//! Canonical memory model for the Barq agentic memory engine.
//!
//! This crate is deliberately free of storage and network dependencies: it
//! defines the vocabulary (taxonomy, records, scopes, queries, errors,
//! configuration) that the engine boundary is frozen around.

pub mod config;
pub mod content;
pub mod error;
pub mod id;
pub mod provenance;
pub mod query;
pub mod record;
pub mod scope;
pub mod subject;
pub mod taxonomy;
pub mod temporal;

pub use config::EngineConfig;
pub use content::MemoryContent;
pub use error::{MemoryError, MemoryResult};
pub use id::MemoryId;
pub use provenance::{Provenance, RetentionClass, RetentionPolicy, SourceKind};
pub use query::MemoryQuery;
pub use record::MemoryRecord;
pub use scope::MemoryScope;
pub use subject::MemorySubject;
pub use taxonomy::{MemoryStatus, MemoryType};
