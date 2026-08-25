//! Query model for exact and filtered lookups.
//!
//! This is the Phase-0 contract for "give me records matching these
//! constraints". Semantic recall arrives in later phases as its own
//! request type built on top of these primitives.

use crate::error::{MemoryError, MemoryResult};
use crate::scope::MemoryScope;
use crate::subject::MemorySubject;
use crate::taxonomy::{MemoryStatus, MemoryType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default result budget when a caller does not specify one.
pub const DEFAULT_QUERY_LIMIT: u32 = 50;

/// Hard ceiling on `limit` to protect providers and callers.
pub const MAX_QUERY_LIMIT: u32 = 1_000;

/// A filtered, non-semantic lookup over canonical records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryQuery {
    /// Scope partition; pinned dimensions must match exactly.
    pub scope: MemoryScope,
    /// Restrict to these types; empty means all types.
    pub memory_types: Vec<MemoryType>,
    /// Restrict to these statuses; defaults to active only.
    pub statuses: Vec<MemoryStatus>,
    /// Optional subject anchor for exact structured lookup.
    pub subject: Option<MemorySubject>,
    /// Optional plain-text keyword filter applied by the provider.
    pub text: Option<String>,
    /// Temporal snapshot: facts valid at this instant. `None` means now.
    pub valid_at: Option<DateTime<Utc>>,
    /// Maximum number of results.
    pub limit: u32,
}

impl Default for MemoryQuery {
    fn default() -> Self {
        Self {
            scope: MemoryScope::default(),
            memory_types: Vec::new(),
            statuses: vec![MemoryStatus::Active],
            subject: None,
            text: None,
            valid_at: None,
            limit: DEFAULT_QUERY_LIMIT,
        }
    }
}

impl MemoryQuery {
    /// Starts a query scoped to nothing (global wildcard).
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins the query scope.
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = scope;
        self
    }

    /// Restricts to one memory type.
    pub fn of_type(mut self, memory_type: MemoryType) -> Self {
        self.memory_types = vec![memory_type];
        self
    }

    /// Restricts to several memory types.
    pub fn of_types(mut self, types: impl IntoIterator<Item = MemoryType>) -> Self {
        self.memory_types = types.into_iter().collect();
        self
    }

    /// Anchors the query on a subject.
    pub fn with_subject(mut self, subject: MemorySubject) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Sets a keyword filter.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Sets the temporal snapshot instant.
    pub fn valid_at(mut self, at: DateTime<Utc>) -> Self {
        self.valid_at = Some(at);
        self
    }

    /// Includes retired statuses (history inspection).
    pub fn include_history(mut self) -> Self {
        self.statuses = MemoryStatus::ALL_STATUSES.to_vec();
        self
    }

    /// Caps results.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// Validates invariants that providers should not each re-check.
    pub fn validated(self) -> MemoryResult<Self> {
        if self.limit == 0 {
            return Err(MemoryError::validation(
                "limit",
                "must be greater than zero",
            ));
        }
        if self.limit > MAX_QUERY_LIMIT {
            return Err(MemoryError::validation(
                "limit",
                format!("must not exceed {MAX_QUERY_LIMIT}"),
            ));
        }
        if let Some(text) = &self.text {
            if text.trim().is_empty() {
                return Err(MemoryError::validation("text", "must not be blank"));
            }
        }
        Ok(self)
    }

    /// Effective snapshot instant (`now` when unset).
    pub fn effective_valid_at(&self) -> DateTime<Utc> {
        self.valid_at.unwrap_or_else(Utc::now)
    }

    /// True when the query accepts the given type.
    pub fn accepts_type(&self, t: MemoryType) -> bool {
        self.memory_types.is_empty() || self.memory_types.contains(&t)
    }

    /// True when the query accepts the given status.
    pub fn accepts_status(&self, s: MemoryStatus) -> bool {
        self.statuses.contains(&s)
    }
}

impl MemoryStatus {
    /// Every lifecycle status, canonical order.
    pub const ALL_STATUSES: [MemoryStatus; 5] = [
        MemoryStatus::Active,
        MemoryStatus::Superseded,
        MemoryStatus::Quarantined,
        MemoryStatus::Archived,
        MemoryStatus::Deleted,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::MemoryScopeBuilder;

    #[test]
    fn defaults_are_conservative() {
        let q = MemoryQuery::new();
        assert_eq!(q.statuses, vec![MemoryStatus::Active]);
        assert_eq!(q.limit, DEFAULT_QUERY_LIMIT);
        assert!(q.memory_types.is_empty());
        assert!(q.accepts_type(MemoryType::Episodic));
        assert!(!q.accepts_status(MemoryStatus::Deleted));
    }

    #[test]
    fn validation_rejects_zero_and_oversized_limits() {
        assert!(MemoryQuery::new().with_limit(0).validated().is_err());
        assert!(
            MemoryQuery::new()
                .with_limit(MAX_QUERY_LIMIT + 1)
                .validated()
                .is_err()
        );
        assert!(MemoryQuery::new().with_limit(1).validated().is_ok());
    }

    #[test]
    fn validation_rejects_blank_text() {
        let q = MemoryQuery::new().with_text("   ");
        assert_eq!(
            q.validated().unwrap_err(),
            MemoryError::validation("text", "must not be blank")
        );
    }

    #[test]
    fn history_mode_expands_statuses() {
        let q = MemoryQuery::new().include_history();
        for s in MemoryStatus::ALL_STATUSES {
            assert!(q.accepts_status(s));
        }
    }

    #[test]
    fn serde_roundtrip_keeps_defaults_on_missing_fields() {
        let q = MemoryQuery::new().of_type(MemoryType::Prospective);
        let json = serde_json::to_string(&q).expect("serialize");
        let back: MemoryQuery = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, MemoryQuery::default().of_type(MemoryType::Prospective));
        let _ = MemoryScopeBuilder::default(); // builder stays reachable via re-export path
    }

    #[test]
    fn effective_snapshot_defaults_to_now() {
        let before = Utc::now();
        let q = MemoryQuery::new();
        assert!(q.effective_valid_at() >= before);
    }
}
