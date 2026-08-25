//! Retention sweeps on the engine facade.

use crate::engine::MemoryEngine;
use chrono::{DateTime, Utc};
use memory_domain::{MemoryId, MemoryResult, MemoryScope};
use memory_lifecycle::{LifecycleProviders, RetentionSweeper, SweepReport};

impl MemoryEngine {
    /// Runs a retention sweep over `scope`, coordinating deletion
    /// across every attached provider (store, vector, graph).
    ///
    /// Returns the audit report. Sweeps are explicit: no background
    /// timer lives in the engine (scale-out phase adds workers).
    pub async fn run_retention_sweep(
        &self,
        scope: &MemoryScope,
        now: DateTime<Utc>,
    ) -> MemoryResult<SweepReport> {
        let sweeper = RetentionSweeper::new(LifecycleProviders {
            store: self.store.clone(),
            vector: self.vector.clone(),
            graph: self.graph.clone(),
        });
        sweeper.sweep(scope, now).await
    }

    /// Coordinated, immediate erasure of one memory from every
    /// representation — the compliance path.
    pub async fn forget_everywhere(&self, id: MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        let sweeper = RetentionSweeper::new(LifecycleProviders {
            store: self.store.clone(),
            vector: self.vector.clone(),
            graph: self.graph.clone(),
        });
        sweeper.coordinate_delete(&id, scope).await
    }
}

#[allow(unused_imports)]
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::RememberRequest;
    use memory_domain::{
        MemoryType, RetentionClass, RetentionPolicy,
        config::{EmbeddingConfig, EngineConfig, VectorStoreConfig},
    };
    use std::sync::Arc;

    async fn engine_with_semantics() -> MemoryEngine {
        let config = EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 64 }),
            ..EngineConfig::default()
        };
        MemoryEngine::from_config(config).await.unwrap()
    }

    #[tokio::test]
    async fn sweep_purges_expired_ephemeral_records_end_to_end() {
        let engine = engine_with_semantics().await;

        let ephemeral = RememberRequest::new(MemoryType::Working, "short-lived scratch")
            .with_retention(RetentionPolicy {
                class: RetentionClass::Ephemeral,
                expires_at: Some(Utc::now() - chrono::Duration::seconds(5)),
            });

        // Seed directly so the record is already expired at insert.
        let saved = {
            let mut r = ephemeral.into_record(engine.config.default_scope.clone());
            r.created_at = Utc::now();
            engine.store.put(&r).await.expect("seed")
        };
        let report = engine
            .run_retention_sweep(&Default::default(), Utc::now())
            .await
            .expect("sweep");

        assert!(report.purged.contains(&saved.id));
        assert!(
            engine
                .recall_exact(saved.id, &Default::default())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn permanent_memories_survive_sweeps() {
        let engine = MemoryEngine::from_config(Default::default()).await.unwrap();
        let mut request = RememberRequest::new(MemoryType::Semantic, "load-bearing fact")
            .with_retention(RetentionPolicy::permanent());
        request.retention = RetentionPolicy::permanent();
        let saved = engine.remember(request).await.expect("remember");

        let report = engine
            .run_retention_sweep(&Default::default(), Utc::now())
            .await
            .unwrap();
        assert_eq!(report.skipped, 1);
        assert!(
            engine
                .recall_exact(saved.id, &Default::default())
                .await
                .unwrap()
                .is_some()
        );
    }
}
