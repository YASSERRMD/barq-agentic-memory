//! Working (active session) state contract.

use async_trait::async_trait;
use memory_domain::MemoryResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::time::Duration;

/// Volatile state an agent needs mid-session.
///
/// Working memory is operational only: it never becomes long-term
/// memory automatically. The revision counter supports version-safe
/// read-modify-write cycles across concurrent tool calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkingMemoryState {
    /// Session this state belongs to.
    pub session_id: String,
    /// Arbitrary JSON payload (recent observations, active goals, ...).
    pub data: Json,
    /// Monotonic revision bumped on every accepted write.
    pub revision: u64,
    /// Last accepted write instant.
    pub updated_at: DateTime<Utc>,
}

impl WorkingMemoryState {
    /// Fresh state at revision 1.
    pub fn initial(session_id: impl Into<String>, data: Json) -> Self {
        Self {
            session_id: session_id.into(),
            data,
            revision: 1,
            updated_at: Utc::now(),
        }
    }

    /// Derives the next revision from the current state.
    ///
    /// Fails when `expected_revision` no longer matches, which callers
    /// translate into a retry with fresh state.
    pub fn derive_next(&self, expected_revision: u64, data: Json) -> MemoryResult<Self> {
        if self.revision != expected_revision {
            return Err(memory_domain::MemoryError::VersionConflict {
                memory_id: memory_domain::MemoryId::generate(),
                expected: expected_revision,
                actual: self.revision,
            });
        }
        Ok(Self {
            session_id: self.session_id.clone(),
            data,
            revision: self.revision + 1,
            updated_at: Utc::now(),
        })
    }
}

/// Fast volatile storage for active session state.
#[async_trait]
pub trait WorkingMemoryProvider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Stores or replaces session state with a time-to-live.
    async fn set(&self, state: &WorkingMemoryState, ttl: Duration) -> MemoryResult<()>;

    /// Reads current state for a session, if present and unexpired.
    async fn get(&self, session_id: &str) -> MemoryResult<Option<WorkingMemoryState>>;

    /// Drops session state immediately. Idempotent.
    async fn delete(&self, session_id: &str) -> MemoryResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn revisions_advance_only_from_matching_expectations() {
        let s = WorkingMemoryState::initial("sess-1", json!({"goal": "deploy"}));
        let next = s.derive_next(1, json!({"goal": "verify"})).expect("next");
        assert_eq!(next.revision, 2);

        let stale = s.derive_next(7, json!({"goal": "x"}));
        assert!(stale.is_err(), "stale expectations must not clobber");
    }

    #[test]
    fn state_roundtrips_through_serde() {
        let s = WorkingMemoryState::initial("sess-9", json!([1, 2, 3]));
        let back: WorkingMemoryState =
            serde_json::from_str(&serde_json::to_string(&s).expect("serialize"))
                .expect("deserialize");
        assert_eq!(back, s);
    }
}
