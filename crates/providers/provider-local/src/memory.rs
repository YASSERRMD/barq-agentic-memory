//! Volatile in-process canonical store.
//!
//! Useful for tests, ephemeral agents, and as the semantic reference
//! for what "provider" means at MVP scope. All filtering goes through
//! [`crate::filter::matches_query`].

use crate::filter::matches_query;
use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope};
use memory_provider_api::MemoryStoreProvider;
use std::collections::HashMap;
use std::sync::RwLock;

/// Thread-safe canonical store that keeps everything in process.
pub struct InMemoryStore {
    namespace: String,
    records: RwLock<HashMap<MemoryId, MemoryRecord>>,
}

impl InMemoryStore {
    /// Creates a store isolated under a logical namespace.
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            records: RwLock::new(HashMap::new()),
        }
    }

    /// Logical namespace of this instance.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Number of stored records regardless of visibility.
    pub fn len(&self) -> usize {
        self.records.read().expect("poisoned").len()
    }

    /// True when no records are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl MemoryStoreProvider for InMemoryStore {
    fn name(&self) -> &str {
        "memory"
    }

    async fn put(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        self.records
            .write()
            .expect("poisoned")
            .insert(memory.id, memory.clone());
        Ok(memory.clone())
    }

    async fn get(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<Option<MemoryRecord>> {
        let guard = self.records.read().expect("poisoned");
        match guard.get(id) {
            Some(record) if scope.contains(&record.scope) => Ok(Some(record.clone())),
            _ => Ok(None),
        }
    }

    async fn update(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        let mut guard = self.records.write().expect("poisoned");
        let slot = guard.get_mut(&memory.id).ok_or(MemoryError::NotFound {
            memory_id: memory.id,
        })?;
        *slot = memory.clone();
        Ok(memory.clone())
    }

    async fn delete(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        let mut guard = self.records.write().expect("poisoned");
        if let Some(record) = guard.get(id) {
            if !scope.contains(&record.scope) {
                return Ok(()); // invisible => already absent for this caller
            }
        }
        guard.remove(id);
        Ok(())
    }

    async fn query(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryRecord>> {
        let query = query.clone().validated()?;
        let guard = self.records.read().expect("poisoned");
        let mut hits: Vec<MemoryRecord> = guard
            .values()
            .filter(|r| matches_query(r, &query))
            .cloned()
            .collect();
        // Newest first gives callers a stable, useful default order.
        hits.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        hits.truncate(query.limit as usize);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use memory_domain::{MemoryContent, MemoryScopeBuilder, MemoryType};

    fn sample(text: &str, memory_type: MemoryType) -> MemoryRecord {
        MemoryRecord::new(memory_type, MemoryContent::from_text(text))
    }

    #[tokio::test]
    async fn put_get_update_delete_roundtrip() {
        let store = InMemoryStore::new("test");
        let mut r = sample("hello", MemoryType::Working);

        store.put(&r).await.expect("put");
        assert_eq!(store.len(), 1);

        r.content = MemoryContent::from_text("updated");
        r.version += 1;
        store.update(&r).await.expect("update");

        let got = store
            .get(&r.id, &MemoryScope::default())
            .await
            .expect("get");
        assert_eq!(got.unwrap().content.text, "updated");

        store
            .delete(&r.id, &MemoryScope::default())
            .await
            .expect("delete");
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn update_missing_record_is_not_found() {
        let store = InMemoryStore::new("test");
        let err = store
            .update(&sample("x", MemoryType::Semantic))
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::NotFound { .. }));
    }

    #[tokio::test]
    async fn delete_is_idempotent_across_scopes() {
        let store = InMemoryStore::new("test");
        let mut r = sample("secret", MemoryType::Semantic);
        r.scope = MemoryScopeBuilder::new().tenant("acme").build();
        store.put(&r).await.expect("put");

        let foreign = MemoryScopeBuilder::new().tenant("other").build();
        store
            .delete(&r.id, &foreign)
            .await
            .expect("invisible delete ok");
        assert_eq!(store.len(), 1, "foreign caller must not remove it");

        store
            .delete(&r.id, &r.scope.clone())
            .await
            .expect("owner deletes");
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn query_filters_by_type_and_text_with_limit() {
        let store = InMemoryStore::new("test");
        for i in 0..5 {
            store
                .put(&sample(&format!("atlas fact {i}"), MemoryType::Semantic))
                .await
                .expect("put");
        }
        for i in 0..3 {
            store
                .put(&sample(&format!("atlas event {i}"), MemoryType::Episodic))
                .await
                .expect("put");
        }

        let q = MemoryQuery::default()
            .of_type(MemoryType::Episodic)
            .with_text("atlas")
            .with_limit(2);
        let hits = store.query(&q).await.expect("query");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.memory_type == MemoryType::Episodic));

        let newest_first = store
            .query(&MemoryQuery::default())
            .await
            .expect("query all");
        assert_eq!(newest_first.len(), 8);
        let times: Vec<_> = newest_first.iter().map(|r| r.created_at).collect();
        let mut sorted = times.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(times, sorted);
    }

    #[tokio::test]
    async fn query_respects_temporal_snapshot() {
        let store = InMemoryStore::new("test");
        let mut r = sample("old fact", MemoryType::Semantic);
        r.valid_to = Some(Utc::now() - Duration::hours(2));
        store.put(&r).await.expect("put");

        let now_q = MemoryQuery::default();
        assert!(store.query(&now_q).await.expect("query").is_empty());

        let past_q = now_q.valid_at(Utc::now() - Duration::hours(3));
        assert_eq!(store.query(&past_q).await.expect("query").len(), 1);
    }
}
