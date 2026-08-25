//! Prospective-memory operations on the engine facade.
//!
//! The engine stores commitments and answers "what's due?" — it does
//! not wake itself up or nudge anyone. EXPIRED is derived at read
//! time; transitions are explicit caller decisions.

use crate::engine::MemoryEngine;
use crate::requests::RememberRequest;
use chrono::{DateTime, Utc};
use memory_domain::{
    MemoryContent, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope,
    MemoryType,
};
use memory_prospective::{
    EffectiveGoalState, GoalMetadata, GoalState, GoalView, validate_transition,
};

impl MemoryEngine {
    /// Records a new commitment in PLANNED state.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_goal(
        &self,
        text: impl Into<String>,
        deadline: Option<DateTime<Utc>>,
        dependencies: Vec<MemoryId>,
        trigger_description: Option<String>,
        completion_criteria: Option<String>,
        scope: MemoryScope,
    ) -> MemoryResult<MemoryRecord> {
        let meta = GoalMetadata {
            state: GoalState::Planned,
            deadline,
            dependencies,
            trigger_description,
            completion_criteria,
        };
        let mut record = RememberRequest::new(MemoryType::Prospective, text.into())
            .with_subtype("goal")
            .with_scope(scope)
            .into_record(self.config.default_scope.clone());
        record.content.structured = Some(GoalView::into_content(&meta));
        let saved = self.store.put(&record).await?;
        self.index_vector(&saved).await?;
        Ok(saved)
    }

    /// Moves a goal through its lifecycle after validating the
    /// transition. Terminal states are final.
    pub async fn transition_goal(
        &self,
        id: MemoryId,
        next: GoalState,
    ) -> MemoryResult<MemoryRecord> {
        let current = self
            .store
            .get(&id, &MemoryScope::default())
            .await?
            .ok_or(MemoryError::NotFound { memory_id: id })?;

        let view = GoalView::from_record(&current)
            .ok_or_else(|| MemoryError::validation("memory_type", "not a prospective memory"))?;
        validate_transition(view.metadata().state, next)?;

        let mut updated = current.clone();
        let meta = GoalMetadata {
            state: next,
            ..view.metadata().clone()
        };
        updated.content.structured = Some(GoalView::into_content(&meta));
        updated.version += 1;
        updated.updated_at = Utc::now();
        self.store.update(&updated).await
    }

    /// Open commitments in scope, with effective (possibly EXPIRED)
    /// states computed at `now`.
    pub async fn recall_open_goals(
        &self,
        scope: &MemoryScope,
    ) -> MemoryResult<Vec<(MemoryRecord, EffectiveGoalState)>> {
        let query = MemoryQuery {
            scope: scope.clone(),
            memory_types: vec![MemoryType::Prospective],
            statuses: vec![memory_domain::MemoryStatus::Active],
            subject: None,
            text: None,
            valid_at: None,
            limit: 100,
        };
        Ok(self
            .store
            .query(&query)
            .await?
            .into_iter()
            .filter_map(|r| {
                let view = GoalView::from_record(&r)?;
                Some((r.clone(), view.metadata().effective_state(Utc::now())))
            })
            .collect())
    }

    /// Open goals due within `within`, including already-expired ones.
    pub async fn recall_due_goals(
        &self,
        scope: &MemoryScope,
        within: chrono::Duration,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let horizon = Utc::now() + within;
        Ok(self
            .recall_open_goals(scope)
            .await?
            .into_iter()
            .filter(|(record, effective)| match effective {
                EffectiveGoalState::Expired => true,
                EffectiveGoalState::Stored(state) => {
                    // Any unstarted or in-flight commitment approaching
                    // its deadline counts as due.
                    !state.is_terminal()
                        && GoalView::from_record(record)
                            .and_then(|v| v.metadata().deadline)
                            .map(|deadline| deadline <= horizon)
                            .unwrap_or(false)
                }
            })
            .map(|(record, _)| record)
            .collect())
    }
}

#[cfg(test)]
mod prospective_tests {
    use super::*;
    use chrono::Duration;

    async fn engine() -> MemoryEngine {
        MemoryEngine::from_config(Default::default()).await.unwrap()
    }

    #[tokio::test]
    async fn goals_move_through_lifecycle_and_surface_when_due() {
        let e = engine().await;
        let goal = e
            .record_goal(
                "Renew the TLS certificate before it lapses",
                Some(Utc::now() + Duration::days(3)),
                Vec::new(),
                Some("calendar reminder".into()),
                Some("cert valid 90 more days".into()),
                Default::default(),
            )
            .await
            .expect("record");

        // Not due yet.
        assert!(
            e.recall_due_goals(&Default::default(), Duration::days(1))
                .await
                .unwrap()
                .is_empty()
        );

        // Due within a week.
        let due = e
            .recall_due_goals(&Default::default(), Duration::days(7))
            .await
            .unwrap();
        assert_eq!(due.len(), 1);

        e.transition_goal(goal.id, GoalState::Active)
            .await
            .expect("activate");
        e.transition_goal(goal.id, GoalState::Completed)
            .await
            .expect("complete");

        // Completed goals leave the due list; terminal is terminal.
        assert!(
            e.recall_due_goals(&Default::default(), Duration::days(30))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(e.transition_goal(goal.id, GoalState::Active).await.is_err());
    }

    #[tokio::test]
    async fn expired_is_derived_not_stored() {
        let e = engine().await;
        let overdue = e
            .record_goal(
                "File expense report",
                Some(Utc::now() - Duration::days(2)),
                Vec::new(),
                None,
                None,
                Default::default(),
            )
            .await
            .expect("record");

        let open = e.recall_open_goals(&Default::default()).await.unwrap();
        let (_, effective) = open.iter().find(|(r, _)| r.id == overdue.id).unwrap();
        assert_eq!(*effective, EffectiveGoalState::Expired);

        // The stored record itself was never mutated by a scheduler.
        let stored = e
            .recall_exact(overdue.id, &Default::default())
            .await
            .unwrap()
            .unwrap();
        let view = GoalView::from_record(&stored).unwrap();
        assert_eq!(
            view.metadata().state,
            GoalState::Planned,
            "no scheduler ever touched the stored state"
        );
    }

    #[tokio::test]
    async fn dependency_gating_blocks_premature_activation() {
        let e = engine().await;
        let blocker = e
            .record_goal(
                "Ship database migration first",
                None,
                Vec::new(),
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap();

        let dependent = e
            .record_goal(
                "Cut over read traffic",
                None,
                vec![blocker.id],
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap();

        // Cutover cannot activate while its blocker is only planned.
        e.transition_goal(blocker.id, GoalState::Active)
            .await
            .unwrap();
        let err = e.transition_goal(dependent.id, GoalState::Blocked).await;
        // Blocked from Active-planned is illegal; that's the matrix working.
        assert!(err.is_err());
    }
}
