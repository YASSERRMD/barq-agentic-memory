//! Planner-backed recall entry points on the engine facade.
//!
//! Phase 5 exposes planning only; full hybrid execution lands in
//! phase 6. Exposing the plan lets callers inspect and override routing
//! before any provider call happens.

use memory_retrieval::{RecallMode, RecallRequest, RetrievalPlan, RuleBasedPlanner};

use crate::engine::MemoryEngine;
use memory_domain::MemoryResult;

impl MemoryEngine {
    /// Compiles a recall request into a retrieval plan without running
    /// any lookup.
    pub fn plan_recall(&self, request: &RecallRequest) -> MemoryResult<RetrievalPlan> {
        if request.mode == RecallMode::SemanticOnly && !self.supports_semantic_recall() {
            return Err(memory_domain::MemoryError::Unsupported(
                "semantic-only recall requires a vector backend".into(),
            ));
        }
        RuleBasedPlanner.plan(request)
    }
}

#[cfg(test)]
mod planning_tests {
    use super::*;
    use memory_domain::MemoryError;
    use memory_domain::config::{EmbeddingConfig, EngineConfig, VectorStoreConfig};

    #[tokio::test]
    async fn plans_reflect_available_backends() {
        let plain = MemoryEngine::from_config(EngineConfig::default())
            .await
            .expect("plain engine");

        let r = RecallRequest::new("What database does Atlas use?")
            .with_subject(memory_domain::MemorySubject::new("atlas"));
        let plan = plain.plan_recall(&r).expect("plan");
        assert!(plan.steps.len() >= 2, "exact first, vector fallback");
    }

    #[tokio::test]
    async fn semantic_only_requires_a_vector_backend() {
        let plain = MemoryEngine::from_config(EngineConfig::default())
            .await
            .expect("plain engine");
        let r = RecallRequest::new("preferences?").with_mode(RecallMode::SemanticOnly);
        assert!(matches!(
            plain.plan_recall(&r),
            Err(MemoryError::Unsupported(_))
        ));

        let semantic = MemoryEngine::from_config(EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 64 }),
            ..EngineConfig::default()
        })
        .await
        .expect("semantic engine");
        assert!(semantic.plan_recall(&r).is_ok());
    }
}
