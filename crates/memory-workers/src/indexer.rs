//! The async indexing worker: keeps the vector index in step with the
//! canonical store off the synchronous write path.
//!
//! `MemoryEngine::remember` indexes inline (correctness first); at
//! scale, deployments disable inline indexing and let this worker
//! reconcile instead — the blueprint's rule that non-critical indexing
//! never blocks writers.

use crate::worker::Worker;
use memory_core::MemoryEngine;
use memory_domain::MemoryResult;
use std::sync::Arc;
use std::time::Duration;

/// Periodically repairs the vector index against canonical truth.
pub struct IndexingWorker {
    engine: Arc<MemoryEngine>,
    interval: Duration,
}

impl IndexingWorker {
    /// Wraps an engine (must have vector + embedder configured).
    pub fn new(engine: Arc<MemoryEngine>) -> Self {
        Self {
            engine,
            interval: Duration::from_secs(30),
        }
    }

    /// Overrides the cadence.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

#[async_trait::async_trait]
impl Worker for IndexingWorker {
    fn name(&self) -> &str {
        "indexing"
    }

    async fn run_once(&self) -> MemoryResult<()> {
        let (_ghosts, reindexed) = self.engine.repair_vector_index().await?;
        let _ = reindexed; // observability consumes these in server mode
        Ok(())
    }

    fn interval(&self) -> Duration {
        self.interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_core::RememberRequest;
    use memory_domain::MemoryType;
    use memory_domain::config::{EmbeddingConfig, EngineConfig, VectorStoreConfig};

    #[tokio::test]
    async fn worker_reindexes_records_that_missed_inline_indexing() {
        let engine = MemoryEngine::from_config(EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 64 }),
            ..EngineConfig::default()
        })
        .await
        .unwrap();
        let engine = Arc::new(engine);

        // Store bypassing inline indexing — exactly what async-mode
        // writers do. EngineConfig still owns defaults; this is the
        // documented off-path write helper.
        let record = engine
            .write_unindexed(RememberRequest::new(
                MemoryType::Semantic,
                "worker must index me",
            ))
            .await
            .unwrap();

        let worker = IndexingWorker::new(engine.clone()).with_interval(Duration::from_secs(1));
        worker.run_once().await.expect("run");

        let hits = engine
            .recall_semantic("worker must index", 5, &Default::default())
            .await
            .unwrap();
        assert!(
            hits.iter().any(|h| h.record.id == record.id),
            "indexed by the worker"
        );
    }
}
