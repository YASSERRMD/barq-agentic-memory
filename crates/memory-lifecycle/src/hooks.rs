//! Archival hooks invoked before records leave the hot path.

use async_trait::async_trait;
use memory_domain::MemoryRecord;

/// Observed when a record is archived or purged.
///
/// Implementations might ship copies to object storage, forward to
/// compliance pipelines, or simply log. Hooks must not fail sweeps:
/// errors are reported per-record but never abort the run.
#[async_trait]
pub trait ArchivalHook: Send + Sync {
    fn name(&self) -> &str;

    /// Called before an archived/purged record disappears from
    /// default retrieval.
    async fn on_archive(&self, record: &MemoryRecord) -> memory_domain::MemoryResult<()>;
}

/// Trivial logging hook; useful as a default and in tests.
pub struct LogArchiveHook;

#[async_trait]
impl ArchivalHook for LogArchiveHook {
    fn name(&self) -> &str {
        "log"
    }

    async fn on_archive(&self, record: &MemoryRecord) -> memory_domain::MemoryResult<()> {
        // Structured logging arrives with observability (phase 21);
        // this keeps the hook contract exercised end-to-end.
        let _ = (&record.id, &record.status);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_hook_succeeds_for_any_record() {
        let r = MemoryRecord::new(
            memory_domain::MemoryType::Working,
            memory_domain::MemoryContent::from_text("x"),
        );
        LogArchiveHook.on_archive(&r).await.expect("hook");
    }
}
