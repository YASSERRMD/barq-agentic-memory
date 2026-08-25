//! Conflict analysis and temporal-truth resolution.
//!
//! Older facts are never silently destroyed: contradiction triggers
//! supersession (window closed, record kept as history) or quarantine
//! for review — the choice is driven by authority and confidence, and
//! every decision carries its evidence.

pub mod resolution;

pub use resolution::{
    ConflictAnalysis, ConflictKind, ResolutionPolicy, SupersessionOutcome,
};
