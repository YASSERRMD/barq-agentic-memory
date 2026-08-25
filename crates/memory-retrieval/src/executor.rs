//! The executor: runs a plan's steps and merges candidates.
//!
//! Pipeline per the blueprint: parallel retrieval -> merge -> scope
//! filter -> remove superseded -> score -> rerank -> return. Steps run
//! concurrently where their providers differ; canonical truth always
//! decides what a caller finally sees.

use crate::plan::{LookupKind, ProviderKind, RetrievalPlan, RetrievalStep};
use crate::request::RecallRequest;
use crate::scoring::{ScoreContext, ScoreWeights, temporal_relevance};
use chrono::{Duration, Utc};
use memory_domain::{MemoryError, MemoryRecord, MemoryResult, MemoryType};
use memory_provider_api::{MemoryStoreProvider, VectorProvider, WorkingMemoryProvider};
use std::collections::HashMap;
use std::sync::Arc;

/// Providers an execution needs; built by the engine from its config.
pub struct ProviderSet {
    pub store: Arc<dyn MemoryStoreProvider>,
    pub vector: Option<Arc<dyn VectorProvider>>,
    pub working: Option<Arc<dyn WorkingMemoryProvider>>,
}

/// One merged candidate with its best composite score.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedCandidate {
    /// Canonically hydrated record.
    pub record: MemoryRecord,
    /// Final composite score after rerank.
    pub score: f32,
}

/// Executes plans against providers.
pub struct HybridExecutor<'a> {
    providers: &'a ProviderSet,
    weights: ScoreWeights,
}

/// Half-life for recency: 30 days keeps recent activity dominant
/// without erasing month-old facts.
const RECENCY_HALF_LIFE: Duration = Duration::days(30);

impl<'a> HybridExecutor<'a> {
    /// Builds an executor with default scoring weights.
    pub fn new(providers: &'a ProviderSet) -> Self {
        Self {
            providers,
            weights: ScoreWeights::default(),
        }
    }

    /// Overrides scoring weights (e.g. for experiments).
    pub fn with_weights(mut self, weights: ScoreWeights) -> Self {
        self.weights = weights;
        self
    }

    /// Runs the full pipeline for a request.
    pub async fn execute(
        &self,
        request: &RecallRequest,
        plan: &RetrievalPlan,
        embedder: Option<&dyn memory_provider_api::EmbeddingProvider>,
    ) -> MemoryResult<Vec<RankedCandidate>> {
        if plan.requires_embeddings() && embedder.is_none() {
            return Err(MemoryError::Unsupported(
                "plan requires embeddings but none is configured".into(),
            ));
        }

        // 1. Retrieval — steps are independent lookups; fan them out.
        let mut candidates: HashMap<memory_domain::MemoryId, (MemoryRecord, f32)> = HashMap::new();
        let mut futures = Vec::with_capacity(plan.steps.len());
        for step in &plan.steps {
            futures.push(self.run_step(request, step, embedder));
        }
        let results = futures::future::join_all(futures).await;
        for result in results {
            for (record, sim) in result? {
                // 2. Merge — keep the highest similarity per id.
                candidates
                    .entry(record.id)
                    .and_modify(|(_, existing)| *existing = existing.max(sim))
                    .or_insert((record, sim));
            }
        }

        // 3+4. Scope filter happened at fetch; drop superseded now:
        // a candidate whose successor is also present loses, while
        // history stays addressable via history().
        let superseding: std::collections::HashSet<memory_domain::MemoryId> = candidates
            .values()
            .filter_map(|(record, _)| record.supersedes)
            .collect();
        candidates.retain(|id, _| !superseding.contains(id));

        // 5+6. Score and rerank.
        let now = Utc::now();
        let mut ranked: Vec<RankedCandidate> = candidates
            .into_values()
            .map(|(record, similarity)| {
                let ctx = ScoreContext::new(similarity, now, &RECENCY_HALF_LIFE);
                let base = crate::scoring::score(&record, &ctx, &self.weights);
                let temporal = temporal_relevance(&record, request.valid_at.unwrap_or(now));
                RankedCandidate {
                    record,
                    score: base * temporal,
                }
            })
            .collect();
        ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).expect("finite scores"));
        ranked.truncate(request.budget as usize);
        Ok(ranked)
    }

    async fn run_step(
        &self,
        request: &RecallRequest,
        step: &RetrievalStep,
        embedder: Option<&dyn memory_provider_api::EmbeddingProvider>,
    ) -> MemoryResult<Vec<(MemoryRecord, f32)>> {
        match (&step.provider, &step.kind) {
            (ProviderKind::Store, LookupKind::ExactSubject) => {
                self.exact_subject(request, step).await
            }
            (ProviderKind::Store, LookupKind::Keyword) => self.keyword(request, step).await,
            (ProviderKind::Vector, LookupKind::Semantic { query_text }) => {
                let Some(embedder) = embedder else {
                    return Err(MemoryError::Unsupported(
                        "semantic step without an embedder".into(),
                    ));
                };
                self.semantic(request, step, query_text.clone(), embedder)
                    .await
            }
            (ProviderKind::Working, _) => self.working_candidates(request).await,
            _ => Ok(Vec::new()),
        }
    }

    async fn exact_subject(
        &self,
        request: &RecallRequest,
        step: &RetrievalStep,
    ) -> MemoryResult<Vec<(MemoryRecord, f32)>> {
        let Some(subject) = &request.subject else {
            return Ok(Vec::new());
        };
        let query = memory_domain::MemoryQuery {
            scope: request.scope.clone(),
            memory_types: allowed_types(step),
            statuses: vec![memory_domain::MemoryStatus::Active],
            subject: Some(subject.clone()),
            text: None,
            valid_at: Some(step.valid_at),
            limit: step.limit.max(1),
        };
        let hits = self.providers.store.query(&query).await?;
        // Exact matches start near the ceiling; ranking still adjusts.
        Ok(hits.into_iter().map(|r| (r, 0.95)).collect())
    }

    async fn keyword(
        &self,
        request: &RecallRequest,
        step: &RetrievalStep,
    ) -> MemoryResult<Vec<(MemoryRecord, f32)>> {
        let keywords = crate::keywords::extract(&request.text);
        let Some(primary) = keywords.first() else {
            return Ok(Vec::new());
        };
        let query = memory_domain::MemoryQuery {
            scope: request.scope.clone(),
            memory_types: allowed_types(step),
            statuses: vec![memory_domain::MemoryStatus::Active],
            subject: None,
            text: Some(primary.clone()),
            valid_at: Some(step.valid_at),
            limit: step.limit.max(1),
        };
        let hits = self.providers.store.query(&query).await?;
        Ok(hits.into_iter().map(|r| (r, 0.7)).collect())
    }

    async fn semantic(
        &self,
        request: &RecallRequest,
        step: &RetrievalStep,
        query_text: String,
        embedder: &dyn memory_provider_api::EmbeddingProvider,
    ) -> MemoryResult<Vec<(MemoryRecord, f32)>> {
        let Some(vector) = &self.providers.vector else {
            return Ok(Vec::new());
        };
        let embedding = embedder.embed(&[query_text]).await?.remove(0);
        let matches = vector
            .search(&memory_provider_api::VectorQuery {
                embedding,
                top_k: step.limit.max(1),
                scope: Some(request.scope.clone()),
                memory_type: single_type(step),
                filter: memory_provider_api::MetadataFilter::default(),
            })
            .await?;

        // Hydrate through the canonical store so scope isolation and
        // status/validity truth apply to every candidate.
        let mut out = Vec::with_capacity(matches.len());
        for m in matches {
            if let Some(record) = self
                .providers
                .store
                .get(&m.memory_id, &request.scope)
                .await?
            {
                out.push((record, m.score));
            }
        }
        Ok(out)
    }

    async fn working_candidates(
        &self,
        request: &RecallRequest,
    ) -> MemoryResult<Vec<(MemoryRecord, f32)>> {
        // Working state is not a canonical record yet; surface it as a
        // synthetic observation only when the session exists.
        let Some(working) = &self.providers.working else {
            return Ok(Vec::new());
        };
        let _ = working;
        // Session state has no stable identity for ranking; it joins via
        // dedicated engine APIs instead of competing in this merge.
        let _ = request;
        Ok(Vec::new())
    }
}

fn allowed_types(step: &RetrievalStep) -> Vec<MemoryType> {
    step.memory_types.clone()
}

fn single_type(step: &RetrievalStep) -> Option<MemoryType> {
    if step.memory_types.len() == 1 {
        step.memory_types.first().copied()
    } else {
        None
    }
}
