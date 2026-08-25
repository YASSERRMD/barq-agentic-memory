//! Planner- and executor-backed recall on the engine facade.
//!
//! recall() compiles intent into a plan (phase 5), runs it across the
//! configured providers, and returns canonically-hydrated, reranked
//! results (phase 6). plan_recall() remains for inspection and tests.

use memory_retrieval::{
    HybridExecutor, ProviderSet, RankedCandidate, RecallMode, RecallRequest, RetrievalPlan,
    RuleBasedPlanner,
};

use crate::engine::MemoryEngine;
use memory_domain::{MemoryResult, MemoryScope};

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

    /// Full hybrid recall: plan -> execute -> merge -> rank.
    ///
    /// This is the primary read path agents use; it consults every
    /// configured backend according to the plan and returns at most
    /// `request.budget` candidates.
    pub async fn recall(&self, request: &RecallRequest) -> MemoryResult<Vec<RankedCandidate>> {
        let plan = self.plan_recall(request)?;
        let providers = ProviderSet {
            store: self.store.clone(),
            vector: self.vector.clone(),
            working: Some(self.working.clone()),
        };
        let embedder = self.embedder.as_deref();
        let executor = HybridExecutor::new(&providers);
        executor.execute(request, &plan, embedder).await
    }
}

#[cfg(test)]
mod planning_tests {
    use super::*;
    use crate::{RememberRequest, UpdateRequest};
    use memory_domain::config::{EmbeddingConfig, EngineConfig, VectorStoreConfig};
    use memory_domain::{MemoryError, MemoryScopeBuilder, MemoryType};

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

    async fn hybrid_engine() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
            ..EngineConfig::default()
        })
        .await
        .expect("hybrid engine")
    }

    #[tokio::test]
    async fn recall_executes_plan_and_reranks_canonical_records() {
        let engine = hybrid_engine().await;

        let atlas = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "Project Atlas uses PostgreSQL")
                    .with_subject(memory_domain::MemorySubject::new("atlas").with_type("project"))
                    .from_source(memory_domain::SourceKind::User, "u-1"),
            )
            .await
            .expect("remember atlas");
        engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "The kitchen fridge needs restocking",
            ))
            .await
            .expect("remember kitchen");

        let hits = engine
            .recall(
                &RecallRequest::new("What database does Project Atlas use?")
                    .with_subject(memory_domain::MemorySubject::new("atlas").with_type("project"))
                    .with_budget(5),
            )
            .await
            .expect("recall");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].record.id, atlas.id);
        assert!(hits[0].score > 0.0);
        assert!(
            hits.iter()
                .all(|h| h.record.status == memory_domain::MemoryStatus::Active),
            "only live facts compete in recall"
        );
    }

    #[tokio::test]
    async fn recall_prefers_successors_over_predecessors() {
        let engine = hybrid_engine().await;
        let v1 = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "Atlas uses MySQL",
            ))
            .await
            .expect("v1");
        let v2 = engine
            .update(UpdateRequest::content(
                v1.id,
                MemoryScope::default(),
                "Atlas uses PostgreSQL",
            ))
            .await
            .expect("v2");

        let hits = engine
            .recall(&RecallRequest::new("which database does atlas use").with_budget(10))
            .await
            .expect("recall");
        assert!(
            hits.iter().all(|h| h.record.id != v1.id),
            "superseded predecessor must not compete with its successor"
        );
        assert!(hits.iter().any(|h| h.record.id == v2.id));
    }

    #[tokio::test]
    async fn recall_respects_scope_isolation_end_to_end() {
        let engine = hybrid_engine().await;
        let acme = MemoryScopeBuilder::new().tenant("acme").build();

        engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "secret acme roadmap details")
                    .with_scope(acme.clone()),
            )
            .await
            .expect("remember");

        let globex = MemoryScopeBuilder::new().tenant("globex").build();
        let foreign = engine
            .recall(
                &RecallRequest::new("acme roadmap details")
                    .with_scope(globex)
                    .with_budget(10),
            )
            .await
            .expect("foreign recall");
        assert!(
            foreign.is_empty(),
            "unauthorized memories must never surface"
        );
    }
}
