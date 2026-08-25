//! Deduplication signals and decisions.
//!
//! Embedding similarity is one signal among many — never sufficient
//! alone. The decision cascade weighs structural signals (hashes,
//! canonical keys, temporal overlap) first because they are cheap,
//! deterministic, and explainable.

pub mod decision;
pub mod engine;
pub mod signals;

pub use decision::{DedupAction, DedupDecision};
pub use engine::DedupEngine;
pub use signals::text_fingerprint;
