//! Health, index repair, and graceful degradation on the engine.
//!
//! Blueprint failure mode: "Qdrant unavailable -> PostgreSQL exact
//! retrieval remains available." Health reports what still serves;
//! repairs reconcile the vector index with canonical truth.

use crate::engine::MemoryEngine;
use memory_domain::{MemoryError, MemoryQuery, MemoryResult, MemoryScope, MemoryType};

use memory_reliability::{Health, HealthStatus};

/// Aggregate health of the engine's backends.
pub struct EngineHealth {
    /// Canonical store health (primary).
    pub store: Health,
    /// Vector index health (secondary; degraded ≠ down).
    pub vector: Option<Health>,
    /// Working memory health.
    pub working: Health,
}

impl EngineHealth {
    /// Worst-case status across components.
    pub fn overall(&self) -> HealthStatus {
        let mut all = vec![self.store.status, self.working.status];
        if let Some(v) = &self.vector {
            all.push(v.status);
        }
        HealthStatus::worst(&all)
    }

    /// True when exact retrieval remains available even if secondary
    /// backends are degraded — the graceful-degradation contract.
    pub fn exact_retrieval_available(&self) -> bool {
        self.store.status != HealthStatus::Unhealthy
    }
}

impl MemoryEngine {
    /// Probes every attached backend.
    pub async fn health(&self) -> MemoryResult<EngineHealth> {
        // Store: a tiny scoped query proves read-path viability.
        let store = match self
            .store
            .query(&MemoryQuery {
                scope: MemoryScope::default(),
                memory_types: vec![MemoryType::Semantic],
                ..Default::default()
            })
            .await
        {
            Ok(_) => Health::ok("store"),
            Err(e) => Health::down("store", e.to_string()),
        };

        let vector = match &self.vector {
            None => None,
            Some(v) => match v
                .search(&memory_provider_api::VectorQuery {
                    embedding: vec![0.0],
                    top_k: 1,
                    ..Default::default()
                })
                .await
            {
                Ok(_) => Some(Health::ok("vector")),
                // A failing vector backend degrades recall, not reads.
                Err(e) => Some(Health::degraded("vector", e.to_string())),
            },
        };

        let working = match self.working.get("healthz-probe").await {
            Ok(_) => Health::ok("working"),
            Err(e) => Health::degraded("working", e.to_string()),
        };

        Ok(EngineHealth {
            store,
            vector,
            working,
        })
    }

    /// Reconciles the vector index with canonical truth.
    ///
    /// - Ghost vectors (index entry, no canonical record) are deleted.
    /// - Missing vectors (active record, no index entry) are re-indexed.
    ///
    /// Returns (ghosts_removed, reindexed). Safe to run repeatedly.
    pub async fn repair_vector_index(&self) -> MemoryResult<(usize, usize)> {
        let Some(vector) = &self.vector else {
            return Err(MemoryError::Unsupported(
                "no vector backend attached".into(),
            ));
        };
        let Some(embedder) = &self.embedder else {
            return Err(MemoryError::Unsupported("no embedder configured".into()));
        };

        let active = self
            .store
            .query(&MemoryQuery {
                scope: MemoryScope::default(),
                memory_types: Vec::new(),
                statuses: vec![memory_domain::MemoryStatus::Active],
                subject: None,
                text: None,
                valid_at: None,
                limit: 1_000,
            })
            .await?;

        let indexed_ids: std::collections::HashSet<memory_domain::MemoryId> =
            vector.list_ids().await?.into_iter().collect();
        let active_ids: std::collections::HashSet<memory_domain::MemoryId> =
            active.iter().map(|r| r.id).collect();

        // Ghosts: indexed but not canonically active.
        let mut ghosts_removed = 0usize;
        for id in indexed_ids.difference(&active_ids) {
            vector.delete(id).await?;
            ghosts_removed += 1;
        }

        // Missing: active records the index never saw (or lost).
        let missing: Vec<&memory_domain::MemoryRecord> = active
            .iter()
            .filter(|r| !indexed_ids.contains(&r.id))
            .collect();
        if !missing.is_empty() {
            let texts: Vec<String> = missing.iter().map(|r| r.content.text.clone()).collect();
            let embeddings = embedder.embed(&texts).await?;
            for (record, embedding) in missing.iter().zip(embeddings) {
                let mut vr = memory_provider_api::VectorRecord::new(
                    record.id,
                    embedding,
                    embedder.model(),
                    embedder.model_version(),
                );
                for (k, v) in Self::scope_filter(&record.scope).equals {
                    vr.metadata.insert(k, v);
                }
                vector.upsert(&vr).await?;
            }
        }

        Ok((ghosts_removed, missing.len()))
    }
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use crate::RememberRequest;
    use memory_domain::config::{EmbeddingConfig, EngineConfig, VectorStoreConfig};

    async fn engine() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 64 }),
            ..EngineConfig::default()
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn health_reports_healthy_and_supports_degradation() {
        let engine = engine().await;
        let health = engine.health().await.expect("health");
        assert_eq!(health.overall(), HealthStatus::Healthy);
        assert!(health.exact_retrieval_available());
        assert!(health.vector.is_some());
    }

    #[tokio::test]
    async fn repair_removes_ghosts_and_reindexes_missing() {
        let engine = engine().await;

        // Seed two records normally (indexed).
        let a = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "indexed fact one",
            ))
            .await
            .unwrap();
        let b_id = {
            let b = engine
                .remember(RememberRequest::new(
                    MemoryType::Semantic,
                    "indexed fact two",
                ))
                .await
                .unwrap();
            b.id
        };

        // Manufacture a ghost: purge the canonical row only.
        engine
            .store
            .delete(&b_id, &Default::default())
            .await
            .unwrap();

        // Manufacture a missing vector: store directly, bypassing remember.
        let orphan = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "will lose its vector",
            ))
            .await
            .unwrap();
        engine
            .vector
            .as_ref()
            .unwrap()
            .delete(&orphan.id)
            .await
            .unwrap();

        let (ghosts, reindexed) = engine.repair_vector_index().await.expect("repair");
        assert_eq!(ghosts, 1, "purged record's vector is a ghost");
        assert_eq!(reindexed, 1, "orphaned record gets re-indexed");

        // After repair, the orphan is recallable again.
        let hits = engine
            .recall_semantic("lose its vector", 5, &Default::default())
            .await
            .unwrap();
        assert!(hits.iter().any(|h| h.record.id == orphan.id));
        let _ = a.id;
    }
}
