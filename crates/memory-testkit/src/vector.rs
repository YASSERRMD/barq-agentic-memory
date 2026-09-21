//! Recording vector index with injectable failures.

use crate::failures::{FailureCounters, FailureKnobs, err};
use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryResult};
use memory_provider_api::{
    VectorMatch, VectorProvider, VectorQuery, VectorRecord, cosine_similarity,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// One recorded vector interaction, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorCall {
    Upsert(MemoryId),
    Search,
    Delete(MemoryId),
    ListIds,
}

/// Mock vector index mirroring the in-memory provider's semantics.
pub struct MockVector {
    vectors: Mutex<HashMap<MemoryId, VectorRecord>>,
    calls: Mutex<Vec<VectorCall>>,
    knobs: FailureKnobs,
    counters: FailureCounters,
}

impl MockVector {
    pub fn shared(knobs: FailureKnobs) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            vectors: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            knobs,
            counters: FailureCounters::default(),
        })
    }

    pub fn calls(&self) -> Vec<VectorCall> {
        self.calls.lock().expect("poisoned").clone()
    }

    pub fn saw(&self, call: &VectorCall) -> bool {
        self.calls().contains(call)
    }

    pub fn len(&self) -> usize {
        self.vectors.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn record(&self, call: VectorCall) {
        self.calls.lock().expect("poisoned").push(call);
    }
}

#[async_trait]
impl VectorProvider for MockVector {
    fn name(&self) -> &str {
        "mock-vector"
    }

    async fn upsert(&self, record: &VectorRecord) -> MemoryResult<()> {
        self.record(VectorCall::Upsert(record.memory_id));
        if self.knobs.put.tripped(&self.counters.put) {
            return Err(err("upsert"));
        }
        self.vectors
            .lock()
            .expect("poisoned")
            .insert(record.memory_id, record.clone());
        Ok(())
    }

    async fn search(&self, query: &VectorQuery) -> MemoryResult<Vec<VectorMatch>> {
        self.record(VectorCall::Search);
        if self.knobs.query.tripped(&self.counters.query) {
            return Err(err("search"));
        }
        let q = query.clone().validated()?;
        let guard = self.vectors.lock().expect("poisoned");
        let mut hits: Vec<VectorMatch> = guard
            .values()
            .filter(|r| query.filter.matches(&r.metadata) && r.embedding.len() == q.embedding.len())
            .map(|r| VectorMatch {
                memory_id: r.memory_id,
                score: cosine_similarity(&q.embedding, &r.embedding),
            })
            .filter(|m| m.score > 0.0)
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .expect("finite scores")
                .then_with(|| a.memory_id.cmp(&b.memory_id))
        });
        hits.truncate(q.top_k as usize);
        Ok(hits)
    }

    async fn delete(&self, memory_id: &MemoryId) -> MemoryResult<()> {
        self.record(VectorCall::Delete(*memory_id));
        if self.knobs.delete.tripped(&self.counters.delete) {
            return Err(err("delete"));
        }
        self.vectors.lock().expect("poisoned").remove(memory_id);
        Ok(())
    }

    async fn list_ids(&self) -> MemoryResult<Vec<MemoryId>> {
        self.record(VectorCall::ListIds);
        if self.knobs.get.tripped(&self.counters.get) {
            return Err(err("list_ids"));
        }
        Ok(self
            .vectors
            .lock()
            .expect("poisoned")
            .keys()
            .copied()
            .collect())
    }
}

#[allow(unused)]
fn _unused(_: MemoryError) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: MemoryId, vec: Vec<f32>) -> VectorRecord {
        VectorRecord::new(id, vec, "mock", "1")
    }

    #[tokio::test]
    async fn upsert_search_delete_recorded_in_order() {
        let index = MockVector::shared(FailureKnobs::none());
        let a = MemoryId::generate();
        let b = MemoryId::generate();
        index.upsert(&record(a, vec![1.0, 0.0])).await.unwrap();
        index.upsert(&record(b, vec![0.9, 0.1])).await.unwrap();

        let hits = index
            .search(&VectorQuery {
                embedding: vec![1.0, 0.0],
                top_k: 5,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].memory_id, a, "closest first");

        index.delete(&b).await.unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(
            index.calls(),
            vec![
                VectorCall::Upsert(a),
                VectorCall::Upsert(b),
                VectorCall::Search,
                VectorCall::Delete(b),
            ]
        );
    }

    #[tokio::test]
    async fn search_failure_injection_for_degradation_tests() {
        let index = MockVector::shared(FailureKnobs {
            query: crate::FailPlan::Always,
            ..FailureKnobs::none()
        });
        assert!(
            index
                .search(&VectorQuery {
                    embedding: vec![1.0],
                    top_k: 1,
                    ..Default::default()
                })
                .await
                .is_err()
        );
    }
}
