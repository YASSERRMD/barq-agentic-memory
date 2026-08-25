//! Goal lifecycle over canonical records.

use chrono::{DateTime, Utc};
use memory_domain::{MemoryError, MemoryId, MemoryRecord, MemoryResult, MemoryType};
use serde::{Deserialize, Serialize};

/// Commitment lifecycle states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    Planned,
    Active,
    Waiting,
    Blocked,
    Completed,
    Cancelled,
}

impl GoalState {
    pub const OPEN_STATES: [GoalState; 4] = [
        GoalState::Planned,
        GoalState::Active,
        GoalState::Waiting,
        GoalState::Blocked,
    ];

    /// Terminal states stay terminal.
    pub fn is_terminal(&self) -> bool {
        matches!(self, GoalState::Completed | GoalState::Cancelled)
    }

    fn can_transition_to(&self, next: GoalState) -> bool {
        use GoalState::*;
        matches!(
            (self, next),
            (Planned, Active)
                | (Planned, Cancelled)
                | (Active, Waiting)
                | (Active, Blocked)
                | (Active, Completed)
                | (Active, Cancelled)
                | (Waiting, Active)
                | (Waiting, Blocked)
                | (Waiting, Cancelled)
                | (Blocked, Active)
                | (Blocked, Cancelled)
        )
    }
}

/// Derived state for open goals past their deadline.
///
/// EXPIRED is deliberately not a stored state: deriving it at read time
/// means the engine needs no scheduler to notice deadlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveGoalState {
    Stored(GoalState),
    Expired,
}

/// Structured metadata in the record's `content.structured`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalMetadata {
    pub state: GoalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
    /// Goals or memories this one depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<MemoryId>,
    /// What should bring this goal back to attention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_description: Option<String>,
    /// How the agent (or a human) knows it is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criteria: Option<String>,
}

impl GoalMetadata {
    /// Computes the effective state at `now`, deriving EXPIRED for
    /// open goals whose deadline has passed.
    pub fn effective_state(&self, now: DateTime<Utc>) -> EffectiveGoalState {
        if let Some(deadline) = self.deadline {
            if deadline <= now && !self.state.is_terminal() {
                return EffectiveGoalState::Expired;
            }
        }
        EffectiveGoalState::Stored(self.state)
    }
}

/// Typed view over a prospective canonical record.
pub struct GoalView<'a> {
    record: &'a MemoryRecord,
    metadata: GoalMetadata,
}

impl<'a> GoalView<'a> {
    pub fn from_record(record: &'a MemoryRecord) -> Option<GoalView<'a>> {
        if record.memory_type != MemoryType::Prospective {
            return None;
        }
        let metadata: GoalMetadata =
            serde_json::from_value(record.content.structured.clone()?).ok()?;
        Some(Self { record, metadata })
    }

    pub fn into_content(metadata: &GoalMetadata) -> serde_json::Value {
        serde_json::to_value(metadata).expect("serializable")
    }

    pub fn metadata(&self) -> &GoalMetadata {
        &self.metadata
    }

    pub fn record(&self) -> &MemoryRecord {
        self.record
    }

    /// True when every dependency has reached `completed` among
    /// `resolved` goal records supplied by the caller.
    pub fn dependencies_satisfied(&self, resolved: &[(MemoryId, GoalState)]) -> bool {
        self.metadata.dependencies.iter().all(|dep| {
            resolved
                .iter()
                .any(|(id, state)| id == dep && *state == GoalState::Completed)
        })
    }
}

/// Validates a lifecycle transition.
pub fn validate_transition(current: GoalState, next: GoalState) -> MemoryResult<()> {
    if current.can_transition_to(next) {
        Ok(())
    } else {
        Err(MemoryError::validation(
            "goal_state",
            format!("illegal transition {current:?} -> {next:?}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use memory_domain::MemoryContent;

    fn goal_record(state: GoalState, deadline: Option<DateTime<Utc>>) -> MemoryRecord {
        let meta = GoalMetadata {
            state,
            deadline,
            dependencies: Vec::new(),
            trigger_description: Some("next standup".into()),
            completion_criteria: Some("ticket closed".into()),
        };
        MemoryRecord::new(
            MemoryType::Prospective,
            MemoryContent::from_text("Renew TLS certificate")
                .with_structured(GoalView::into_content(&meta)),
        )
    }

    #[test]
    fn planned_to_completed_requires_activation_path() {
        assert!(validate_transition(GoalState::Planned, GoalState::Active).is_ok());
        assert!(validate_transition(GoalState::Active, GoalState::Completed).is_ok());
        assert!(validate_transition(GoalState::Planned, GoalState::Completed).is_err());
        assert!(validate_transition(GoalState::Completed, GoalState::Active).is_err());
        assert!(validate_transition(GoalState::Waiting, GoalState::Blocked).is_ok());
        assert!(validate_transition(GoalState::Blocked, GoalState::Cancelled).is_ok());
    }

    #[test]
    fn expiry_is_derived_never_stored() {
        let past = Utc::now() - Duration::days(1);
        let overdue = GoalMetadata {
            state: GoalState::Active,
            deadline: Some(past),
            dependencies: Vec::new(),
            trigger_description: None,
            completion_criteria: None,
        };
        assert_eq!(
            overdue.effective_state(Utc::now()),
            EffectiveGoalState::Expired
        );

        // Terminal states keep their identity even past deadlines.
        let mut done = overdue.clone();
        done.state = GoalState::Completed;
        assert_eq!(
            done.effective_state(Utc::now()),
            EffectiveGoalState::Stored(GoalState::Completed)
        );

        let mut future = overdue.clone();
        future.deadline = Some(Utc::now() + Duration::days(7));
        assert_eq!(
            future.effective_state(Utc::now()),
            EffectiveGoalState::Stored(GoalState::Active)
        );
    }

    #[test]
    fn dependency_satisfaction_checks_exact_ids() {
        let view_meta = GoalMetadata {
            state: GoalState::Blocked,
            deadline: None,
            dependencies: vec![MemoryId::generate(), MemoryId::generate()],
            trigger_description: None,
            completion_criteria: None,
        };
        let r = MemoryRecord::new(
            MemoryType::Prospective,
            MemoryContent::from_text("blocked goal")
                .with_structured(GoalView::into_content(&view_meta)),
        );
        let v = GoalView::from_record(&r).expect("view");

        let half_done: Vec<(MemoryId, GoalState)> =
            vec![(view_meta.dependencies[0], GoalState::Completed)];
        assert!(!v.dependencies_satisfied(&half_done));

        let all_done: Vec<(MemoryId, GoalState)> = view_meta
            .dependencies
            .iter()
            .map(|id| (*id, GoalState::Completed))
            .collect();
        assert!(v.dependencies_satisfied(&all_done));
    }

    #[test]
    fn views_reject_non_prospective_records() {
        let r = goal_record(GoalState::Planned, None);
        let v = GoalView::from_record(&r);
        assert!(v.is_some());

        let mut semantic = r.clone();
        semantic.memory_type = MemoryType::Semantic;
        assert!(GoalView::from_record(&semantic).is_none());
    }
}
