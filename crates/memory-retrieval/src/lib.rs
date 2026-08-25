//! Retrieval planning and execution for the Barq memory engine.
//!
//! The planner turns a caller's recall intent into an ordered, inspectable
//! plan of lookup steps; the executor runs those steps against providers
//! and merges the candidates.

pub mod plan;
pub mod planner;
pub mod request;

pub use plan::{LookupKind, ProviderKind, RetrievalPlan, RetrievalStep};
pub use planner::RuleBasedPlanner;
pub use request::{RecallMode, RecallRequest};
