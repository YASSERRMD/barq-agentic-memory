//! Episodic memory: what happened, when, and how it went.
//!
//! Episodes are first-class records of agent experience — action,
//! outcome, feedback, trajectory — linked to canonical memories as
//! evidence. The engine stores and retrieves episodes; it does not
//! interpret them.

pub mod episode;
pub mod store;

pub use episode::Episode;
pub use store::{EpisodeQuery, EpisodeStore, InMemoryEpisodeStore};
