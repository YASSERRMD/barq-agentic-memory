//! Provider registry and engine assembly for the Barq memory engine.
//!
//! The core owns no backends: it resolves configuration into provider
//! trait objects supplied by feature crates.

pub mod assembly;
pub mod registry;

pub use assembly::{AssemblyPlan, ensure_satisfiable};
pub use registry::{ProviderCapability, ProviderRegistry};
