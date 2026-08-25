//! In-process working-memory storage with per-entry TTL.
//!
//! Expiry is lazy (checked on access) plus a sweep method lifecycle
//! phases can call on a schedule; no background timer threads are
//! spawned implicitly.

use async_trait::async_trait;
use memory_domain::MemoryResult;
use memory_provider_api::{WorkingMemoryProvider, WorkingMemoryState};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

struct LiveEntry {
    state: WorkingMemoryState,
    expires_at: Instant,
}

/// Volatile session-state store for embedded deployments.
pub struct InProcessWorkingStore {
    namespace: String,
    entries: RwLock<HashMap<String, LiveEntry>>,
}

impl InProcessWorkingStore {
    /// Creates a store isolated under a logical namespace.
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Logical namespace of this instance.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Removes every expired entry; returns how many were dropped.
    ///
    /// Intended to be called by a maintenance loop, not on the hot path.
    pub fn sweep_expired(&self) -> usize {
        let now = Instant::now();
        let mut guard = self.entries.write().expect("poisoned");
        let before = guard.len();
        guard.retain(|_, entry| entry.expires_at > now);
        before - guard.len()
    }

    /// Number of currently live sessions.
    pub fn live_sessions(&self) -> usize {
        let now = Instant::now();
        self.entries
            .read()
            .expect("poisoned")
            .values()
            .filter(|e| e.expires_at > now)
            .count()
    }

    fn key(&self, session_id: &str) -> String {
        format!("{}:{session_id}", self.namespace)
    }
}

#[async_trait]
impl WorkingMemoryProvider for InProcessWorkingStore {
    fn name(&self) -> &str {
        "in-process"
    }

    async fn set(&self, state: &WorkingMemoryState, ttl: Duration) -> MemoryResult<()> {
        let key = self.key(&state.session_id);
        let entry = LiveEntry {
            state: state.clone(),
            expires_at: Instant::now() + ttl,
        };
        self.entries.write().expect("poisoned").insert(key, entry);
        Ok(())
    }

    async fn get(&self, session_id: &str) -> MemoryResult<Option<WorkingMemoryState>> {
        let key = self.key(session_id);
        let now = Instant::now();
        let mut guard = self.entries.write().expect("poisoned");
        match guard.get(&key) {
            Some(entry) if entry.expires_at > now => Ok(Some(entry.state.clone())),
            Some(_) => {
                guard.remove(&key);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, session_id: &str) -> MemoryResult<()> {
        let key = self.key(session_id);
        self.entries.write().expect("poisoned").remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn set_get_delete_roundtrip() {
        let store = InProcessWorkingStore::new("ns");

        let state = WorkingMemoryState::initial("s-1", json!({"goal": "book flight"}));
        store.set(&state, Duration::from_secs(60)).await.expect("set");

        let got = store.get("s-1").await.expect("get").expect("live");
        assert_eq!(got.data["goal"], "book flight");

        store.delete("s-1").await.expect("delete");
        assert!(store.get("s-1").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn ttl_expires_entries() {
        let store = InProcessWorkingStore::new("ns");
        let state = WorkingMemoryState::initial("s-2", json!({}));
        store
            .set(&state, Duration::from_millis(20))
            .await
            .expect("set");
        assert_eq!(store.live_sessions(), 1);

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(store.get("s-2").await.expect("get").is_none());
        assert_eq!(store.live_sessions(), 0);
    }

    #[tokio::test]
    async fn namespaces_do_not_collide() {
        let a = InProcessWorkingStore::new("agent-a");
        let b = InProcessWorkingStore::new("agent-b");

        let state = WorkingMemoryState::initial("shared-session", json!("a"));
        a.set(&state, Duration::from_secs(60)).await.expect("set");

        assert!(b.get("shared-session").await.expect("get").is_none());
        assert!(a.get("shared-session").await.expect("get").is_some());
    }

    #[tokio::test]
    async fn sweep_removes_only_dead_entries() {
        let store = InProcessWorkingStore::new("ns");
        let dead = WorkingMemoryState::initial("dead", json!({}));
        let alive = WorkingMemoryState::initial("alive", json!({}));
        store.set(&dead, Duration::from_millis(10)).await.expect("set");
        store.set(&alive, Duration::from_secs(60)).await.expect("set");

        tokio::time::sleep(Duration::from_millis(30)).await;
        let removed = store.sweep_expired();
        assert_eq!(removed, 1);
        assert!(store.get("alive").await.expect("get").is_some());
    }
}
