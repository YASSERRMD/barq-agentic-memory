//! Provider registry and engine assembly for the Barq memory engine.
//!
//! The core owns no backends: it resolves configuration into provider
//! trait objects supplied by feature crates.

pub mod assembly;
pub mod engine;
pub mod planning;
pub mod registry;
pub mod requests;

pub use assembly::{AssemblyPlan, ensure_satisfiable};
pub use engine::MemoryEngine;
pub use registry::{ProviderCapability, ProviderRegistry};
pub use requests::{RememberRequest, UpdateRequest};
