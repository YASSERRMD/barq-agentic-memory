//! Recording working-memory provider with failure injection.

use crate::failures::{FailureCounters, FailureKnobs, err};
use async_trait::async_trait;
use memory_domain::MemoryResult;
use memory_provider_api::{WorkingMemoryProvider, WorkingMemoryState};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// One recorded working-memory interaction, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkingCall {
    Set(String),
    Get(String),
    Delete(String),
    Cas(String, u64),
}

/// Mock session store; no real TTL expiry (tests control time), but
/// full revision bookkeeping so CAS semantics hold.
pub struct MockWorking {
    sessions: Mutex<HashMap<String, WorkingMemoryState>>,
    calls: Mutex<Vec<WorkingCall>>,
    knobs: FailureKnobs,
    counters: FailureCounters,
}

impl MockWorking {
    pub fn shared(knobs: FailureKnobs) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            knobs,
            counters: FailureCounters::default(),
        })
    }

    pub fn calls(&self) -> Vec<WorkingCall> {
        self.calls.lock().expect("poisoned").clone()
    }

    fn record(&self, call: WorkingCall) {
        self.calls.lock().expect("poisoned").push(call);
    }
}

#[async_trait]
impl WorkingMemoryProvider for MockWorking {
    fn name(&self) -> &str {
        "mock-working"
    }

    async fn set(&self, state: &WorkingMemoryState, _ttl: Duration) -> MemoryResult<()> {
        self.record(WorkingCall::Set(state.session_id.clone()));
        if self.knobs.put.tripped(&self.counters.put) {
            return Err(err("set"));
        }
        self.sessions
            .lock()
            .expect("poisoned")
            .insert(state.session_id.clone(), state.clone());
        Ok(())
    }

    async fn get(&self, session_id: &str) -> MemoryResult<Option<WorkingMemoryState>> {
        self.record(WorkingCall::Get(session_id.to_string()));
        if self.knobs.get.tripped(&self.counters.get) {
            return Err(err("get"));
        }
        Ok(self
            .sessions
            .lock()
            .expect("poisoned")
            .get(session_id)
            .cloned())
    }

    async fn delete(&self, session_id: &str) -> MemoryResult<()> {
        self.record(WorkingCall::Delete(session_id.to_string()));
        if self.knobs.delete.tripped(&self.counters.delete) {
            return Err(err("delete"));
        }
        self.sessions.lock().expect("poisoned").remove(session_id);
        Ok(())
    }

    async fn compare_and_set(
        &self,
        session_id: &str,
        expected_revision: u64,
        data: Json,
        _ttl: Duration,
    ) -> MemoryResult<WorkingMemoryState> {
        self.record(WorkingCall::Cas(session_id.to_string(), expected_revision));
        if self.knobs.update.tripped(&self.counters.update) {
            return Err(err("cas"));
        }
        let mut guard = self.sessions.lock().expect("poisoned");
        let current =
            guard
                .get_mut(session_id)
                .ok_or(memory_domain::MemoryError::SessionNotFound {
                    session_id: session_id.to_string(),
                })?;
        if current.revision != expected_revision {
            return Err(memory_domain::MemoryError::SessionConflict {
                session_id: session_id.to_string(),
                expected: expected_revision,
                actual: current.revision,
            });
        }
        current.data = data;
        current.revision += 1;
        current.updated_at = chrono::Utc::now();
        Ok(current.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn cas_rejects_stale_revisions_and_accepts_fresh_ones() {
        let store = MockWorking::shared(FailureKnobs::none());
        let state = WorkingMemoryState::initial("s", json!({"step": 1}));
        store.set(&state, Duration::from_secs(60)).await.unwrap();

        let stale = store
            .compare_and_set("s", 7, json!({}), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(matches!(
            stale,
            memory_domain::MemoryError::SessionConflict {
                expected: 7,
                actual: 1,
                ..
            }
        ));

        let next = store
            .compare_and_set("s", 1, json!({"step": 2}), Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(next.revision, 2);
    }

    #[tokio::test]
    async fn calls_recorded_in_order() {
        let store = MockWorking::shared(FailureKnobs::none());
        store
            .set(
                &WorkingMemoryState::initial("x", json!({})),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        store.get("x").await.unwrap();
        store.delete("x").await.unwrap();
        assert_eq!(
            store.calls(),
            vec![
                WorkingCall::Set("x".into()),
                WorkingCall::Get("x".into()),
                WorkingCall::Delete("x".into()),
            ]
        );
    }
}
