//! Procedural memory: instructions and runbooks as governed documents.
//!
//! Procedures live as canonical records (`MemoryType::Procedural`)
//! whose structured payload carries lifecycle metadata. The engine
//! retrieves procedures; it never executes them.

pub mod procedure;

pub use procedure::{ProcedureMetadata, ProcedureState, ProcedureView, validate_transition};
