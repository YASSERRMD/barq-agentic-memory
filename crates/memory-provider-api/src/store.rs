//! Canonical record storage contract.

use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope};

/// Authoritative storage for canonical [`MemoryRecord`]s.
///
/// Implementations must enforce scope isolation: a record is only
/// visible through a scope that `contains` the record's own scope.
#[async_trait]
pub trait MemoryStoreProvider: Send + Sync {
    /// Human-readable provider name for logs and errors.
    fn name(&self) -> &str;

    /// Inserts a new canonical record.
    ///
    /// Providers may reject records whose content or limits violate
    /// engine policy; identity collisions are backend bugs and surface
    /// as [`MemoryError::Storage`].
    async fn put(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord>;

    /// Fetches one record by id within the given scope.
    async fn get(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<Option<MemoryRecord>>;

    /// Replaces the stored version of an already-existing record.
    async fn update(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord>;

    /// Removes a record; idempotent per scope isolation rules.
    async fn delete(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()>;

    /// Filtered lookup over canonical records.
    ///
    /// The default implementation returns "unsupported" so minimal
    /// backends (e.g. pure KV caches) stay valid providers; richer
    /// stores override this with real filtering.
    async fn query(&self, _query: &MemoryQuery) -> MemoryResult<Vec<MemoryRecord>> {
        Err(MemoryError::Unsupported(
            "provider does not implement filtered queries".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryContent, MemoryScopeBuilder, MemoryType};
    use std::sync::Mutex;

    #[derive(Default)]
    struct TinyStore(Mutex<Vec<MemoryRecord>>);

    #[async_trait]
    impl MemoryStoreProvider for TinyStore {
        fn name(&self) -> &str {
            "tiny"
        }

        async fn put(&self, m: &MemoryRecord) -> MemoryResult<MemoryRecord> {
            self.0.lock().expect("lock").push(m.clone());
            Ok(m.clone())
        }

        async fn get(
            &self,
            id: &MemoryId,
            scope: &MemoryScope,
        ) -> MemoryResult<Option<MemoryRecord>> {
            Ok(self
                .0
                .lock()
                .expect("lock")
                .iter()
                .find(|m| m.id == *id && scope.contains(&m.scope))
                .cloned())
        }

        async fn update(&self, m: &MemoryRecord) -> MemoryResult<MemoryRecord> {
            let mut guard = self.0.lock().expect("lock");
            match guard.iter_mut().find(|x| x.id == m.id) {
                Some(slot) => {
                    *slot = m.clone();
                    Ok(m.clone())
                }
                None => Err(MemoryError::NotFound { memory_id: m.id }),
            }
        }

        async fn delete(&self, id: &MemoryId, _scope: &MemoryScope) -> MemoryResult<()> {
            self.0.lock().expect("lock").retain(|m| m.id != *id);
            Ok(())
        }
    }

    #[tokio::test]
    async fn trait_object_dispatch_roundtrips_a_record() {
        let store: std::sync::Arc<dyn MemoryStoreProvider> =
            std::sync::Arc::new(TinyStore::default());
        let scope = MemoryScopeBuilder::new().tenant("t").build();
        let mut record = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("hello"));
        record.scope = scope.clone();

        let put = store.put(&record).await.expect("put");
        assert_eq!(put.id, record.id);

        let got = store.get(&record.id, &scope).await.expect("get");
        assert!(got.is_some());

        let missing_scope = MemoryScopeBuilder::new().tenant("other").build();
        let blocked = store.get(&record.id, &missing_scope).await.expect("get");
        assert!(blocked.is_none(), "scope isolation hides foreign records");

        store.delete(&record.id, &scope).await.expect("delete");
        assert!(store.get(&record.id, &scope).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn default_query_is_unsupported_for_minimal_backends() {
        let store: std::sync::Arc<dyn MemoryStoreProvider> =
            std::sync::Arc::new(TinyStore::default());
        let err = store.query(&MemoryQuery::new()).await.unwrap_err();
        assert!(matches!(err, MemoryError::Unsupported(_)));
    }
}
