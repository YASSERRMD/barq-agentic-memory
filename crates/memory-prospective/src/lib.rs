//! Prospective memory: goals, commitments, and triggers.
//!
//! The engine stores and surfaces commitments; it never wakes itself
//! up. EXPIRED is derived from the deadline at read time — no
//! background scheduler lives in the engine.

pub mod goal;

pub use goal::{EffectiveGoalState, GoalMetadata, GoalState, GoalView, validate_transition};
