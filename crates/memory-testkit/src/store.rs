//! Recording in-memory store with per-method failure injection.

use crate::failures::{FailureCounters, FailureKnobs, err};
use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope};
use memory_provider_api::MemoryStoreProvider;
use provider_local::filter::matches_query;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// One recorded store interaction, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum StoreCall {
    Put(MemoryId),
    Get(MemoryId),
    Update(MemoryId),
    Delete(MemoryId),
    Query,
}

/// Mock canonical store: real filtering semantics, zero I/O.
pub struct MockStore {
    records: Mutex<HashMap<MemoryId, MemoryRecord>>,
    calls: Mutex<Vec<StoreCall>>,
    knobs: FailureKnobs,
    counters: FailureCounters,
}

impl MockStore {
    pub fn new() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            knobs: FailureKnobs::none(),
            counters: FailureCounters::default(),
        }
    }

    /// Shared handle (Arc) with failure knobs applied.
    pub fn shared(knobs: FailureKnobs) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            knobs,
            counters: FailureCounters::default(),
        })
    }

    /// Ordered call log.
    pub fn calls(&self) -> Vec<StoreCall> {
        self.calls.lock().expect("poisoned").clone()
    }

    /// True when `call` appears in the log.
    pub fn saw(&self, call: &StoreCall) -> bool {
        self.calls().contains(call)
    }

    /// Number of stored records regardless of visibility.
    pub fn len(&self) -> usize {
        self.records.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn record(&self, call: StoreCall) {
        self.calls.lock().expect("poisoned").push(call);
    }
}

impl Default for MockStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStoreProvider for MockStore {
    fn name(&self) -> &str {
        "mock-store"
    }

    async fn put(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        self.record(StoreCall::Put(memory.id));
        if self.knobs.put.tripped(&self.counters.put) {
            return Err(err("put"));
        }
        self.records
            .lock()
            .expect("poisoned")
            .insert(memory.id, memory.clone());
        Ok(memory.clone())
    }

    async fn get(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<Option<MemoryRecord>> {
        self.record(StoreCall::Get(*id));
        if self.knobs.get.tripped(&self.counters.get) {
            return Err(err("get"));
        }
        let guard = self.records.lock().expect("poisoned");
        Ok(guard.get(id).filter(|r| scope.contains(&r.scope)).cloned())
    }

    async fn update(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        self.record(StoreCall::Update(memory.id));
        if self.knobs.update.tripped(&self.counters.update) {
            return Err(err("update"));
        }
        let mut guard = self.records.lock().expect("poisoned");
        guard
            .get_mut(&memory.id)
            .map(|slot| {
                *slot = memory.clone();
                memory.clone()
            })
            .ok_or(MemoryError::NotFound {
                memory_id: memory.id,
            })
    }

    async fn delete(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        self.record(StoreCall::Delete(*id));
        if self.knobs.delete.tripped(&self.counters.delete) {
            return Err(err("delete"));
        }
        let mut guard = self.records.lock().expect("poisoned");
        if let Some(record) = guard.get(id) {
            if !scope.contains(&record.scope) {
                return Ok(());
            }
        }
        guard.remove(id);
        Ok(())
    }

    async fn query(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryRecord>> {
        self.record(StoreCall::Query);
        if self.knobs.query.tripped(&self.counters.query) {
            return Err(err("query"));
        }
        let query = query.clone().validated()?;
        let guard = self.records.lock().expect("poisoned");
        let mut hits: Vec<MemoryRecord> = guard
            .values()
            .filter(|r| matches_query(r, &query))
            .cloned()
            .collect();
        hits.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        hits.truncate(query.limit as usize);
        Ok(hits)
    }
}

// Silence unused-import lint pattern when counters used via tripped only.
#[allow(unused)]
fn _ordering_marker(_: Ordering) {}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryContent, MemoryType};

    fn rec(text: &str) -> MemoryRecord {
        MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text(text))
    }

    #[tokio::test]
    async fn records_calls_and_filters_like_production() {
        let store = MockStore::shared(FailureKnobs::none());
        let r = rec("atlas uses postgres");
        store.put(&r).await.unwrap();
        assert_eq!(
            store.calls(),
            vec![StoreCall::Put(r.id)],
            "call log is ordered and complete"
        );

        let hits = store
            .query(&MemoryQuery::default().with_text("postgres"))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(store.saw(&StoreCall::Query));
    }

    #[tokio::test]
    async fn failure_injection_recovers_after_n() {
        let store = MockStore::shared(FailureKnobs {
            put: crate::FailPlan::Times(1),
            ..FailureKnobs::none()
        });
        let r = rec("first");
        assert!(store.put(&r).await.is_err(), "first put fails");
        assert!(
            store.put(&rec("second")).await.is_ok(),
            "second put recovers"
        );
        assert_eq!(store.len(), 1, "only the recovered write landed");
    }

    #[tokio::test]
    async fn scope_isolation_holds() {
        let store = MockStore::shared(FailureKnobs::none());
        let mut r = rec("tenant secret");
        r.scope = memory_domain::MemoryScopeBuilder::new()
            .tenant("acme")
            .build();
        store.put(&r).await.unwrap();

        let foreign = memory_domain::MemoryScopeBuilder::new()
            .tenant("globex")
            .build();
        assert!(store.get(&r.id, &foreign).await.unwrap().is_none());
        store.delete(&r.id, &foreign).await.unwrap();
        assert_eq!(store.len(), 1, "foreign delete is a no-op");
    }
}
