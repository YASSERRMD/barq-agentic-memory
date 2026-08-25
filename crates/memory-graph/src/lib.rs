//! Entity-relation graph memory.
//!
//! Graph records always reference canonical memory ids as evidence —
//! the graph is an index over truth, never a second source of it.
//! Neo4j and other backends implement [`GraphProvider`]; the in-memory
//! store keeps embedded mode working today.

pub mod extractor;
pub mod graph;
pub mod store;

pub use extractor::{RelationExtractor, RuleBasedRelationExtractor};
pub use graph::{Entity, EntityKey, Relation};
pub use store::{GraphProvider, InMemoryGraphStore};
