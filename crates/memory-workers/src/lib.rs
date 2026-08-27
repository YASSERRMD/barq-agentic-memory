//! Background workers separating non-critical work from request paths.
//!
//! Blueprint rule: keep non-critical indexing work off the synchronous
//! write path. These workers own exactly that work — indexing,
//! lifecycle sweeps, index repair — driven by a shared ticker that
//! process managers (server mode, embeddings sidecars) embed.

pub mod indexer;
pub mod ticker;
pub mod worker;

pub use indexer::IndexingWorker;
pub use ticker::WorkerTicker;
pub use worker::{Worker, WorkerRegistry};
