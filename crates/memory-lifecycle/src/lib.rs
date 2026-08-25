//! Lifecycle: retention sweeps and coordinated forgetting.
//!
//! Deletion must invalidate every representation of a memory —
//! canonical row, vector index, graph edges, caches. The sweep is the
//! only component allowed to physically remove records that engine
//! writes merely tombstoned.

pub mod hooks;
pub mod sweep;

pub use hooks::{ArchivalHook, LogArchiveHook};
pub use sweep::{SweepReport, RetentionSweeper};
