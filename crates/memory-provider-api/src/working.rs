//! Working (active session) state contract.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_domain::MemoryResult;
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
            return Err(memory_domain::MemoryError::SessionConflict {
                session_id: self.session_id.clone(),
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

    /// Revision-safe update: applies only if the stored revision still
    /// equals `expected_revision`.
    ///
    /// The default is a non-atomic read-check-write; backends with
    /// scripting (Redis) override this for true atomicity so concurrent
    /// tool calls cannot lose each other's updates.
    async fn compare_and_set(
        &self,
        session_id: &str,
        expected_revision: u64,
        data: Json,
        ttl: Duration,
    ) -> MemoryResult<WorkingMemoryState> {
        match self.get(session_id).await? {
            Some(current) => {
                let next = current.derive_next(expected_revision, data)?;
                self.set(&next, ttl).await?;
                Ok(next)
            }
            None => Err(memory_domain::MemoryError::SessionNotFound {
                session_id: session_id.to_string(),
            }),
        }
    }

    /// Creates fresh state atomically only when absent; returns the
    /// stored state either way (initialize-once semantics).
    ///
    /// Default is best-effort get-then-set.
    async fn initialize(
        &self,
        session_id: &str,
        data: Json,
        ttl: Duration,
    ) -> MemoryResult<WorkingMemoryState> {
        if let Some(existing) = self.get(session_id).await? {
            return Ok(existing);
        }
        let state = WorkingMemoryState::initial(session_id.to_string(), data);
        self.set(&state, ttl).await?;
        Ok(state)
    }
}

/// Typed view over the well-known keys inside session-state JSON.
///
/// These are operational conveniences, not new memory types: everything
/// lives inside the session's [`WorkingMemoryState`] payload and never
/// graduates to long-term memory automatically.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Goals the agent committed to for this session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_goals: Vec<String>,
    /// Latest observations from the environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_observations: Vec<String>,
    /// Recent tool invocation results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<String>,
    /// References to durable checkpoints (memory ids or external refs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoint_refs: Vec<String>,
}

/// Cap per list so one hot session cannot grow unbounded.
const SNAPSHOT_LIST_CAP: usize = 50;

impl SessionSnapshot {
    /// Extracts the snapshot from arbitrary state JSON, tolerating
    /// foreign keys and missing fields.
    pub fn from_state_data(data: &Json) -> Self {
        serde_json::from_value(data.clone()).unwrap_or_default()
    }

    /// Merges back into a state JSON object under well-known keys,
    /// preserving any other fields the caller stored there.
    pub fn apply_to(&self, data: &mut Json) {
        if !data.is_object() {
            *data = serde_json::json!({});
        }
        let map = data.as_object_mut().expect("just made it an object");
        let entries = [
            ("active_goals", &self.active_goals),
            ("recent_observations", &self.recent_observations),
            ("tool_results", &self.tool_results),
            ("checkpoint_refs", &self.checkpoint_refs),
        ];
        for (key, list) in entries {
            map.insert(
                key.to_string(),
                serde_json::to_value(list).expect("serializable"),
            );
        }
    }

    /// Appends a goal.
    pub fn push_goal(&mut self, goal: impl Into<String>) {
        push_capped(&mut self.active_goals, goal.into());
    }

    /// Appends an observation, keeping the newest tail.
    pub fn push_observation(&mut self, observation: impl Into<String>) {
        push_capped(&mut self.recent_observations, observation.into());
    }

    /// Appends a tool result, keeping the newest tail.
    pub fn push_tool_result(&mut self, result: impl Into<String>) {
        push_capped(&mut self.tool_results, result.into());
    }

    /// Records a durable checkpoint reference.
    pub fn add_checkpoint_ref(&mut self, reference: impl Into<String>) {
        push_capped(&mut self.checkpoint_refs, reference.into());
    }
}

fn push_capped(list: &mut Vec<String>, item: String) {
    list.push(item);
    if list.len() > SNAPSHOT_LIST_CAP {
        let excess = list.len() - SNAPSHOT_LIST_CAP;
        list.drain(0..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::MemoryError;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn revisions_advance_only_from_matching_expectations() {
        let s = WorkingMemoryState::initial("sess-1", json!({"goal": "deploy"}));
        let next = s.derive_next(1, json!({"goal": "verify"})).expect("next");
        assert_eq!(next.revision, 2);

        let stale = s.derive_next(7, json!({"goal": "x"})).unwrap_err();
        assert!(matches!(
            stale,
            MemoryError::SessionConflict { ref session_id, expected: 7, actual: 1 }
                if session_id == "sess-1"
        ));
    }

    #[test]
    fn state_roundtrips_through_serde() {
        let s = WorkingMemoryState::initial("sess-9", json!([1, 2, 3]));
        let back: WorkingMemoryState =
            serde_json::from_str(&serde_json::to_string(&s).expect("serialize"))
                .expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn snapshot_preserves_foreign_keys_in_state_json() {
        let mut data = json!({"custom_metric": 42});
        let mut snap = SessionSnapshot::default();
        snap.push_goal("finish report");
        snap.apply_to(&mut data);

        assert_eq!(data["custom_metric"], 42);
        assert_eq!(data["active_goals"], json!(["finish report"]));

        let parsed = SessionSnapshot::from_state_data(&data);
        assert_eq!(parsed.active_goals, vec!["finish report".to_string()]);
        assert!(parsed.recent_observations.is_empty());
    }

    #[test]
    fn snapshot_lists_stay_capped() {
        let mut snap = SessionSnapshot::default();
        for i in 0..80 {
            snap.push_observation(format!("obs-{i}"));
        }
        assert_eq!(snap.recent_observations.len(), SNAPSHOT_LIST_CAP);
        assert_eq!(snap.recent_observations.first().unwrap(), "obs-30");
        assert_eq!(snap.recent_observations.last().unwrap(), "obs-79");
    }

    #[tokio::test]
    async fn default_cas_fails_closed_on_missing_session() {
        struct Bare;
        #[async_trait]
        impl WorkingMemoryProvider for Bare {
            fn name(&self) -> &str {
                "bare"
            }
            async fn set(&self, _s: &WorkingMemoryState, _ttl: Duration) -> MemoryResult<()> {
                Ok(())
            }
            async fn get(&self, _session_id: &str) -> MemoryResult<Option<WorkingMemoryState>> {
                Ok(None)
            }
            async fn delete(&self, _session_id: &str) -> MemoryResult<()> {
                Ok(())
            }
        }

        let err = Bare
            .compare_and_set("ghost", 1, json!({}), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MemoryError::SessionNotFound { ref session_id } if session_id == "ghost"
        ));
    }
}
