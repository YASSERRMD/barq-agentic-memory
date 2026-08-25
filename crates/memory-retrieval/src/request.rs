//! The caller-facing recall request.
//!
//! Deliberately small: what the agent wants, where to look, and how
//! much it cares. Everything else is the planner's job.

use chrono::{DateTime, Utc};
use memory_domain::{MemoryScope, MemorySubject, MemoryType};
use serde::{Deserialize, Serialize};

/// How the caller wants candidates gathered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallMode {
    /// Planner decides (default); may mix exact and semantic steps.
    Auto,
    /// Only exact/structured lookups; no similarity scoring.
    ExactOnly,
    /// Only semantic similarity; no keyword/exact pre-pass.
    SemanticOnly,
}

/// A recall intent from an agent or application.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecallRequest {
    /// Free-text query ("What database does Project Atlas use?").
    pub text: String,
    /// Scope partition for visibility.
    pub scope: MemoryScope,
    /// Caller-pinned memory types; empty lets the planner infer.
    pub requested_types: Vec<MemoryType>,
    /// Subject anchor when the question names its target.
    pub subject: Option<MemorySubject>,
    /// Temporal snapshot; `None` means "current truth".
    pub valid_at: Option<DateTime<Utc>>,
    /// Maximum results across the whole plan.
    pub budget: u32,
    /// Whether episodic evidence may be pulled as supporting context.
    pub allow_episodic_evidence: bool,
    /// Strategy constraint; defaults to [`RecallMode::Auto`].
    pub mode: RecallMode,
}

impl RecallRequest {
    /// Minimal auto-planned request.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            scope: MemoryScope::default(),
            requested_types: Vec::new(),
            subject: None,
            valid_at: None,
            budget: 10,
            allow_episodic_evidence: false,
            mode: RecallMode::Auto,
        }
    }

    /// Pins the scope.
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = scope;
        self
    }

    /// Anchors on a subject.
    pub fn with_subject(mut self, subject: MemorySubject) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Pins memory types.
    pub fn of_types(mut self, types: impl IntoIterator<Item = MemoryType>) -> Self {
        self.requested_types = types.into_iter().collect();
        self
    }

    /// Sets the result budget.
    pub fn with_budget(mut self, budget: u32) -> Self {
        self.budget = budget.max(1);
        self
    }

    /// Allows episodic supporting evidence.
    pub fn with_episodic_evidence(mut self) -> Self {
        self.allow_episodic_evidence = true;
        self
    }

    /// Constrains the strategy.
    pub fn with_mode(mut self, mode: RecallMode) -> Self {
        self.mode = mode;
        self
    }

    /// Structural validation shared by planner and executor.
    pub fn validated(&self) -> Result<(), memory_domain::MemoryError> {
        if self.text.trim().is_empty() {
            return Err(memory_domain::MemoryError::validation(
                "text",
                "must not be blank",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::MemoryScopeBuilder;

    #[test]
    fn defaults_are_sane() {
        let r = RecallRequest::new("what changed?");
        assert_eq!(r.mode, RecallMode::Auto);
        assert_eq!(r.budget, 10);
        assert!(r.requested_types.is_empty());
        assert!(!r.allow_episodic_evidence);
    }

    #[test]
    fn blank_text_is_rejected() {
        let r = RecallRequest::new("   ");
        assert!(r.validated().is_err());
    }

    #[test]
    fn budget_floors_at_one() {
        let r = RecallRequest::new("x").with_budget(0);
        assert_eq!(r.budget, 1);
    }

    #[test]
    fn builder_chaining_sets_scope_and_types() {
        let scope = MemoryScopeBuilder::new().tenant("acme").build();
        let r = RecallRequest::new("q")
            .with_scope(scope.clone())
            .of_types([MemoryType::Semantic, MemoryType::Episodic]);
        assert_eq!(r.scope, scope);
        assert_eq!(r.requested_types.len(), 2);
    }
}
