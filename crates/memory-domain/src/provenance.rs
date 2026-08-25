//! Where a memory came from and how long it should be kept.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Kind of origin that produced a memory.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Stated directly by a human user.
    User,
    /// Asserted by an agent during its work.
    Agent,
    /// Produced by engine-internal processes (compaction, summarization).
    System,
    /// Result of a tool invocation.
    Tool,
    /// Imported from outside the agent runtime.
    External,
}

impl SourceKind {
    /// Default authority weight used by conflict resolution and ranking.
    ///
    /// Human statements outrank tool output and agent inference; exact
    /// numbers are tuned in later phases but the ordering is fixed here
    /// so providers can rely on it.
    pub const fn default_authority(&self) -> f32 {
        match self {
            SourceKind::User => 0.9,
            SourceKind::External => 0.8,
            SourceKind::Tool => 0.7,
            SourceKind::System => 0.6,
            SourceKind::Agent => 0.5,
        }
    }
}

/// Provenance trail of a memory record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// What kind of source created this memory.
    pub source: SourceKind,
    /// Identifier of the specific actor (user id, agent name, tool name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    /// Optional URI pointing at the originating artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
    /// Distributed-trace correlation for auditing write paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// When the memory was captured.
    pub recorded_at: DateTime<Utc>,
}

impl Provenance {
    /// Provenance stamped `now` from the given source kind.
    pub fn now(source: SourceKind) -> Self {
        Self {
            source,
            actor_id: None,
            source_uri: None,
            trace_id: None,
            recorded_at: Utc::now(),
        }
    }

    /// Attaches an actor identifier.
    pub fn with_actor(mut self, actor_id: impl Into<String>) -> Self {
        self.actor_id = Some(actor_id.into());
        self
    }

    /// Attaches a trace identifier.
    pub fn with_trace(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }
}

/// How aggressively a memory may be forgotten.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// May be dropped as soon as it expires; no archival.
    Ephemeral,
    /// Kept for the session that produced it.
    Session,
    /// Default long-lived retention under policy sweeps.
    Standard,
    /// Never expired automatically; deletion requires explicit action.
    Permanent,
}

/// Retention metadata attached to every record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Coarse retention class.
    pub class: RetentionClass,
    /// Absolute expiry instant, if any. Lifecycle sweeps use this to
    /// expire records without needing per-record TTL timers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl RetentionPolicy {
    /// Policy used when callers do not specify one.
    pub fn standard() -> Self {
        Self {
            class: RetentionClass::Standard,
            expires_at: None,
        }
    }

    /// Ephemeral policy expiring at the given instant.
    pub fn expiring_at(expires_at: DateTime<Utc>) -> Self {
        Self {
            class: RetentionClass::Ephemeral,
            expires_at: Some(expires_at),
        }
    }

    /// Permanent policy.
    pub fn permanent() -> Self {
        Self {
            class: RetentionClass::Permanent,
            expires_at: None,
        }
    }

    /// True when the policy has lapsed at `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            Some(at) => at <= now,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn authority_orders_sources_sensibly() {
        assert!(SourceKind::User.default_authority() > SourceKind::Agent.default_authority());
        assert!(SourceKind::Tool.default_authority() > SourceKind::Agent.default_authority());
    }

    #[test]
    fn provenance_skips_unset_fields() {
        let p = Provenance::now(SourceKind::User).with_actor("u-1");
        let json = serde_json::to_string(&p).expect("serialize");
        assert!(!json.contains("trace_id"));
        assert!(!json.contains("source_uri"));
        let back: Provenance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.actor_id.as_deref(), Some("u-1"));
    }

    #[test]
    fn expiry_respects_instant_comparison() {
        let now = Utc::now();
        let past = RetentionPolicy::expiring_at(now - Duration::hours(1));
        let future = RetentionPolicy::expiring_at(now + Duration::hours(1));
        let forever = RetentionPolicy::permanent();

        assert!(past.is_expired(now));
        assert!(!future.is_expired(now));
        assert!(!forever.is_expired(now));
    }

    #[test]
    fn retention_class_serializes_snake_case() {
        let json = serde_json::to_string(&RetentionClass::Permanent).expect("serialize");
        assert_eq!(json, "\"permanent\"");
        let back: RetentionClass = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, RetentionClass::Permanent);
    }
}
