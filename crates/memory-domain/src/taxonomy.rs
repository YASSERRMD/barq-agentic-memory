//! Memory taxonomy: functional memory types and record lifecycle status.
//!
//! The engine exposes exactly five cognitive memory types. Operational
//! views (profile, conversation, entity, ...) are scopes or indexes over
//! these types, never new types of their own.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The five functional memory kinds supported by the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Active state for the current turn or task.
    Working,
    /// Past events, interactions, and their outcomes.
    Episodic,
    /// Durable facts, preferences, entities, and relationships.
    Semantic,
    /// Instructions, skills, and reusable procedures.
    Procedural,
    /// Future goals, commitments, and unfinished work.
    Prospective,
}

impl MemoryType {
    /// Every memory type, in canonical order.
    pub const ALL: [MemoryType; 5] = [
        MemoryType::Working,
        MemoryType::Episodic,
        MemoryType::Semantic,
        MemoryType::Procedural,
        MemoryType::Prospective,
    ];

    /// Lowercase snake_case name matching the serialized form.
    pub const fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Working => "working",
            MemoryType::Episodic => "episodic",
            MemoryType::Semantic => "semantic",
            MemoryType::Procedural => "procedural",
            MemoryType::Prospective => "prospective",
        }
    }

    /// True if records of this type are short-lived by nature.
    ///
    /// Working memory is operational state; it never graduates to
    /// long-term memory automatically.
    pub const fn is_transient(&self) -> bool {
        matches!(self, MemoryType::Working)
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle state of a memory record.
///
/// Records are never silently destroyed: superseded facts stay queryable
/// as history, deletions are tombstoned until coordinated cleanup runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Current, valid memory.
    Active,
    /// Replaced by a newer version; retained as history.
    Superseded,
    /// Retained but hidden from normal retrieval pending review.
    Quarantined,
    /// Archived out of the hot path but still addressable.
    Archived,
    /// Tombstone; physical removal happens during lifecycle sweeps.
    Deleted,
}

impl MemoryStatus {
    /// Status assigned to freshly created records.
    pub const fn initial() -> Self {
        MemoryStatus::Active
    }

    /// True when the record should surface in default retrieval.
    pub const fn is_retrievable(&self) -> bool {
        matches!(self, MemoryStatus::Active | MemoryStatus::Archived)
    }

    /// True once the record has left active duty.
    pub const fn is_retired(&self) -> bool {
        !self.is_retrievable() || matches!(self, MemoryStatus::Archived)
    }
}

impl fmt::Display for MemoryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            MemoryStatus::Active => "active",
            MemoryStatus::Superseded => "superseded",
            MemoryStatus::Quarantined => "quarantined",
            MemoryStatus::Archived => "archived",
            MemoryStatus::Deleted => "deleted",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_roundtrip_through_serde() {
        for kind in MemoryType::ALL {
            let json = serde_json::to_string(&kind).expect("serialize");
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: MemoryType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn working_memory_is_the_only_transient_type() {
        assert!(MemoryType::Working.is_transient());
        assert!(!MemoryType::Semantic.is_transient());
        assert_eq!(
            MemoryType::ALL.iter().filter(|t| t.is_transient()).count(),
            1
        );
    }

    #[test]
    fn retrievable_statuses_are_exactly_active_and_archived() {
        assert!(MemoryStatus::Active.is_retrievable());
        assert!(MemoryStatus::Archived.is_retrievable());
        assert!(!MemoryStatus::Superseded.is_retrievable());
        assert!(!MemoryStatus::Quarantined.is_retrievable());
        assert!(!MemoryStatus::Deleted.is_retrievable());
    }

    #[test]
    fn retired_is_superset_of_non_retrievable() {
        assert!(MemoryStatus::Deleted.is_retired());
        assert!(MemoryStatus::Superseded.is_retired());
        assert!(MemoryStatus::Archived.is_retired());
        assert!(!MemoryStatus::Active.is_retired());
    }
}
