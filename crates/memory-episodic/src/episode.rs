//! The episode model.

use chrono::{DateTime, Utc};
use memory_domain::{MemoryId, MemoryScope};
use serde::{Deserialize, Serialize};

/// One lived experience of the agent.
///
/// Episodes are immutable once written; corrections arrive as new
/// episodes referencing their predecessor in the narrative.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: MemoryId,
    pub scope: MemoryScope,
    /// When the event happened in the world (not when recorded).
    pub event_time: DateTime<Utc>,
    /// What the agent did.
    pub action: String,
    /// What happened as a result.
    pub outcome: String,
    /// Did the action achieve its goal?
    pub success: bool,
    /// Human or system feedback attached to the episode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Compressed multi-step story for long trajectories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_summary: Option<String>,
    /// Canonical memories cited as evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<MemoryId>,
    /// Wall-clock duration of the action, milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub recorded_at: DateTime<Utc>,
}

impl Episode {
    /// Starts a builder for a new episode stamped now.
    pub fn builder(action: impl Into<String>, outcome: impl Into<String>) -> EpisodeBuilder {
        EpisodeBuilder {
            episode: Self {
                id: MemoryId::generate(),
                scope: MemoryScope::default(),
                event_time: Utc::now(),
                action: action.into(),
                outcome: outcome.into(),
                success: true,
                feedback: None,
                trajectory_summary: None,
                evidence_refs: Vec::new(),
                duration_ms: None,
                recorded_at: Utc::now(),
            },
        }
    }

    /// True when this episode cites `memory_id` as evidence.
    pub fn cites(&self, memory_id: &MemoryId) -> bool {
        self.evidence_refs.contains(memory_id)
    }
}

/// Fluent builder for [`Episode`].
pub struct EpisodeBuilder {
    episode: Episode,
}

impl EpisodeBuilder {
    /// Pins when the event happened (vs when recorded).
    pub fn at(mut self, event_time: DateTime<Utc>) -> Self {
        self.episode.event_time = event_time;
        self
    }

    /// Marks failure.
    pub fn failed(mut self) -> Self {
        self.episode.success = false;
        self
    }

    /// Attaches feedback.
    pub fn with_feedback(mut self, feedback: impl Into<String>) -> Self {
        self.episode.feedback = Some(feedback.into());
        self
    }

    /// Attaches a trajectory summary for multi-step work.
    pub fn with_trajectory(mut self, summary: impl Into<String>) -> Self {
        self.episode.trajectory_summary = Some(summary.into());
        self
    }

    /// Cites canonical memories as evidence.
    pub fn citing(mut self, refs: impl IntoIterator<Item = MemoryId>) -> Self {
        self.episode.evidence_refs.extend(refs);
        self
    }

    /// Sets duration.
    pub fn lasting_ms(mut self, ms: u64) -> Self {
        self.episode.duration_ms = Some(ms);
        self
    }

    /// Sets scope.
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.episode.scope = scope;
        self
    }

    /// Finishes construction.
    pub fn build(self) -> Episode {
        self.episode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use memory_domain::MemoryScopeBuilder;

    #[test]
    fn builder_defaults_to_success_now() {
        let e = Episode::builder("deploy", "rolled out cleanly").build();
        assert!(e.success);
        assert!(e.feedback.is_none());
        assert!((e.recorded_at - e.event_time).abs() < Duration::seconds(5));
    }

    #[test]
    fn failures_and_evidence_flow_through() {
        let evidence = MemoryId::generate();
        let e = Episode::builder("migrate db", "constraint violation")
            .failed()
            .with_feedback("check the rollback runbook")
            .citing([evidence])
            .lasting_ms(42_000)
            .build();

        assert!(!e.success);
        assert_eq!(e.duration_ms, Some(42_000));
        assert!(e.cites(&evidence));
        assert!(!e.cites(&MemoryId::generate()));
    }

    #[test]
    fn event_time_can_differ_from_recorded_time() {
        let yesterday = Utc::now() - Duration::days(1);
        let e = Episode::builder("backup", "completed").at(yesterday).build();
        assert!(e.event_time < e.recorded_at);
    }

    #[test]
    fn serializes_with_scope_and_skips_empties() {
        let e = Episode::builder("action", "outcome")
            .with_scope(MemoryScopeBuilder::new().tenant("acme").build())
            .build();
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("acme"));
        assert!(!json.contains("feedback"), "unset fields skipped");
        let back: Episode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, e);
    }
}
