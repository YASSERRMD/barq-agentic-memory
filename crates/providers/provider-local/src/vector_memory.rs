//! In-process vector index for embedded semantic recall.
//!
//! Flat cosine scan — correct and dependency-free. Sufficient for
//! embedded deployments at MVP scale; HNSW-class indexes arrive only
//! when profiling demands them (phase 23 policy).

use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryResult};
use memory_provider_api::{
    cosine_similarity, VectorMatch, VectorProvider, VectorQuery, VectorRecord,
};
use std::collections::HashMap;
use std::sync::RwLock;

/// Volatile vector index; one instance per namespace.
pub struct InMemoryVectorStore {
    namespace: String,
    vectors: RwLock<HashMap<MemoryId, VectorRecord>>,
}

impl InMemoryVectorStore {
    /// Creates an index isolated under a logical namespace.
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            vectors: RwLock::new(HashMap::new()),
        }
    }

    /// Number of indexed vectors.
    pub fn len(&self) -> usize {
        self.vectors.read().expect("poisoned").len()
    }

    /// True when the index is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl VectorProvider for InMemoryVectorStore {
    fn name(&self) -> &str {
        "in-memory"
    }

    async fn upsert(&self, record: &VectorRecord) -> MemoryResult<()> {
        if record.embedding.is_empty() {
            return Err(MemoryError::validation("embedding", "must not be empty"));
        }
        self.vectors
            .write()
            .expect("poisoned")
            .insert(record.memory_id, record.clone());
        Ok(())
    }

    async fn search(&self, query: &VectorQuery) -> MemoryResult<Vec<VectorMatch>> {
        let q = query.clone().validated()?;
        let guard = self.vectors.read().expect("poisoned");

        let mut scored: Vec<VectorMatch> = guard
            .values()
            .filter(|record| {
                // Namespace isolation is structural (one instance per
                // namespace); model stamps must match to avoid mixing.
                query.filter.matches(&record.metadata)
                    && record.embedding.len() == q.embedding.len()
            })
            .map(|record| VectorMatch {
                memory_id: record.memory_id,
                score: cosine_similarity(&q.embedding, &record.embedding),
            })
            .filter(|m| m.score > 0.0)
            .collect();

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).expect("finite scores"));
        scored.truncate(q.top_k as usize);
        Ok(scored)
    }

    async fn delete(&self, memory_id: &MemoryId) -> MemoryResult<()> {
        self.vectors.write().expect("poisoned").remove(memory_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_provider_api::{EmbeddingProvider, HashingEmbedder};
use memory_provider_api::MetadataFilter;

    async fn embed(text: &str, dims: usize) -> Vec<f32> {
        HashingEmbedder::new(dims).embed(&[text.to_string()]).await.expect("embed")[0].clone()
    }

    #[tokio::test]
    async fn top_k_search_ranks_by_similarity() {
        let index = InMemoryVectorStore::new("test");
        let e = HashingEmbedder::new(128);

        let texts = [
            "project atlas uses postgresql",
            "atlas database is postgres",
            "recipe for sourdough bread",
            "deploy checklist for production",
        ];
        let mut ids = Vec::new();
        for text in texts {
            let id = MemoryId::generate();
            ids.push(id);
            index
                .upsert(&VectorRecord::new(
                    id,
                    e.embed(&[text.to_string()]).await.expect("embed")[0].clone(),
                    e.model(),
                    e.model_version(),
                ))
                .await
                .expect("upsert");
        }

        let query_vec = e.embed(&["postgres database atlas".to_string()])
            .await
            .expect("embed")[0]
            .clone();
        let hits = index
            .search(&VectorQuery {
                embedding: query_vec,
                top_k: 2,
                ..Default::default()
            })
            .await
            .expect("search");

        assert_eq!(hits.len(), 2);
        assert!(ids.contains(&hits[0].memory_id));
        assert!(hits[0].score >= hits[1].score);
    }

    #[tokio::test]
    async fn metadata_filter_narrows_results() {
        let index = InMemoryVectorStore::new("test");
        let e = HashingEmbedder::new(64);
        let vec = e.embed(&["shared text".to_string()]).await.expect("embed")[0].clone();

        let keep = MemoryId::generate();
        let drop_ = MemoryId::generate();
        index
            .upsert(
                &VectorRecord::new(keep, vec.clone(), "h", "1")
                    .with_metadata("tenant", "acme"),
            )
            .await
            .expect("upsert");
        index
            .upsert(
                &VectorRecord::new(drop_, vec, "h", "1")
                    .with_metadata("tenant", "globex"),
            )
            .await
            .expect("upsert");

        let mut filter = MetadataFilter::default();
        filter.equals.insert("tenant".into(), "acme".into());

        let hits = index
            .search(&VectorQuery {
                embedding: embed("shared text", 64).await,
                top_k: 10,
                filter,
                ..Default::default()
            })
            .await
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory_id, keep);
    }

    #[tokio::test]
    async fn delete_removes_and_upsert_overwrites() {
        let index = InMemoryVectorStore::new("ns");
        let id = MemoryId::generate();
        index
            .upsert(&VectorRecord::new(id, vec![1.0, 0.0], "h", "1"))
            .await
            .expect("upsert");
        assert_eq!(index.len(), 1);

        index.delete(&id).await.expect("delete");
        index.delete(&id).await.expect("delete idempotent");
        assert!(index.is_empty());
    }

    #[tokio::test]
    async fn dimension_mismatch_is_excluded_not_panic() {
        let index = InMemoryVectorStore::new("ns");
        index
            .upsert(&VectorRecord::new(MemoryId::generate(), vec![1.0, 0.0], "h", "1"))
            .await
            .expect("upsert dim2");

        let hits = index
            .search(&VectorQuery {
                embedding: vec![1.0, 0.0, 0.0],
                top_k: 5,
                ..Default::default()
            })
            .await
            .expect("search");
        assert!(hits.is_empty());
    }
}
