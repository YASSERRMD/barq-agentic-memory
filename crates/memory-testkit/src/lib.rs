//! Mock providers for testing the Barq memory engine.
//!
//! Every mock records calls (order-preserving) and supports failure
//! injection per method, so suites can assert both interactions and
//! degradation behavior deterministically — no infrastructure, no
//! sleeps, no flakiness.

pub mod embedder;
pub mod failures;
pub mod store;
pub mod vector;
pub mod working;

pub use embedder::MockEmbedder;
pub use failures::{FailPlan, FailureKnobs};
pub use store::{MockStore, StoreCall};
pub use vector::{MockVector, VectorCall};
pub use working::{MockWorking, WorkingCall};

/// Deterministic classifier/extractor responses live in the classifier
/// mock; authorizer/auditor mocks are intentionally thin and inline in
/// suites (see memory-policy for InMemoryAuditor).
pub mod classifier {
    pub use super::embedder::{MockClassifier, MockExtractor};
}
