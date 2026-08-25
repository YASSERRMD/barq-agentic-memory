//! Procedural-memory operations on the engine facade.
//!
//! Publishing starts at DRAFT; transitions are validated against the
//! blueprint's lifecycle matrix. The engine retrieves procedures and
//! tracks their governance state — it never executes them.

use crate::engine::MemoryEngine;
use crate::requests::RememberRequest;
use memory_domain::{
    MemoryContent, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope,
    MemoryType,
};
use memory_procedural::{ProcedureMetadata, ProcedureState, ProcedureView, validate_transition};

impl MemoryEngine {
    /// Publishes a new procedure document in DRAFT state.
    pub async fn publish_procedure(
        &self,
        text: impl Into<String>,
        owner: impl Into<String>,
        compatibility: Option<String>,
        scope: MemoryScope,
    ) -> MemoryResult<MemoryRecord> {
        let meta = ProcedureMetadata {
            state: ProcedureState::Draft,
            owner: owner.into(),
            compatibility,
            effective_from: None,
            effective_to: None,
        };
        let request = RememberRequest::new(MemoryType::Procedural, text.into())
            .with_subtype("procedure")
            .with_scope(scope);
        let mut record = request.into_record(self.config.default_scope.clone());
        record.content = MemoryContent {
            structured: Some(ProcedureView::into_content(&meta)),
            ..record.content
        };
        let saved = self.store.put(&record).await?;
        self.index_vector(&saved).await?;
        Ok(saved)
    }

    /// Moves a procedure to the next lifecycle state after validating
    /// the transition. The record version bumps; history is preserved.
    pub async fn transition_procedure(
        &self,
        id: MemoryId,
        next: ProcedureState,
    ) -> MemoryResult<MemoryRecord> {
        let current = self
            .store
            .get(&id, &MemoryScope::default())
            .await?
            .ok_or(MemoryError::NotFound { memory_id: id })?;

        let view = ProcedureView::from_record(&current)
            .ok_or_else(|| MemoryError::validation("memory_type", "not a procedural memory"))?;
        validate_transition(view.state(), next)?;

        let mut updated = current.clone();
        let mut meta = ProcedureMetadata {
            state: next,
            owner: view.owner().to_string(),
            compatibility: None,
            effective_from: view
                .record()
                .content
                .structured
                .as_ref()
                .and_then(|s| s.get("effective_from").cloned())
                .and_then(|v| serde_json::from_value(v).ok()),
            effective_to: view
                .record()
                .content
                .structured
                .as_ref()
                .and_then(|s| s.get("effective_to").cloned())
                .and_then(|v| serde_json::from_value(v).ok()),
        };
        if let Some(c) = &view.record().content.structured {
            meta.compatibility = c
                .get("compatibility")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok());
        }
        updated.content.structured = Some(ProcedureView::into_content(&meta));
        updated.version += 1;
        updated.updated_at = chrono::Utc::now();
        self.store.update(&updated).await
    }

    /// Active, currently-effective procedures in scope.
    pub async fn recall_active_procedures(
        &self,
        scope: &MemoryScope,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let query = MemoryQuery {
            scope: scope.clone(),
            memory_types: vec![MemoryType::Procedural],
            statuses: vec![memory_domain::MemoryStatus::Active],
            subject: None,
            text: None,
            valid_at: None,
            limit: 50,
        };
        let hits = self.store.query(&query).await?;
        Ok(hits
            .into_iter()
            .filter(|r| {
                ProcedureView::from_record(r)
                    .map(|v| v.state().is_operative() && v.is_currently_effective())
                    .unwrap_or(false)
            })
            .collect())
    }
}

#[cfg(test)]
mod procedure_tests {
    use super::*;

    #[tokio::test]
    async fn full_lifecycle_from_draft_to_active() {
        let engine = MemoryEngine::from_config(Default::default()).await.unwrap();
        let doc = engine
            .publish_procedure(
                "1. drain nodes\n2. upgrade control plane",
                "platform",
                Some("k8s-1.29".into()),
                Default::default(),
            )
            .await
            .expect("publish");

        engine
            .transition_procedure(doc.id, ProcedureState::Review)
            .await
            .expect("review");
        engine
            .transition_procedure(doc.id, ProcedureState::Approved)
            .await
            .expect("approve");
        let active = engine
            .transition_procedure(doc.id, ProcedureState::Active)
            .await
            .expect("activate");

        assert_eq!(
            active.version, 4,
            "each governance change bumps the revision (v1 + 3 transitions)"
        );

        let live = engine
            .recall_active_procedures(&Default::default())
            .await
            .unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(
            ProcedureView::from_record(&live[0]).unwrap().state(),
            ProcedureState::Active
        );
    }

    #[tokio::test]
    async fn illegal_transitions_are_rejected_with_named_edges() {
        let engine = MemoryEngine::from_config(Default::default()).await.unwrap();
        let doc = engine
            .publish_procedure("steps here", "ops", None, Default::default())
            .await
            .unwrap();

        // Draft cannot jump straight to Active...
        let err = engine
            .transition_procedure(doc.id, ProcedureState::Active)
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Validation { .. }));

        // ...and non-procedures cannot be governed at all.
        let fact = engine
            .remember(crate::RememberRequest::new(
                MemoryType::Semantic,
                "just a fact",
            ))
            .await
            .unwrap();
        assert!(
            engine
                .transition_procedure(fact.id, ProcedureState::Review)
                .await
                .is_err()
        );

        // Draft -> Review is still fine afterwards.
        assert!(
            engine
                .transition_procedure(doc.id, ProcedureState::Review)
                .await
                .is_ok()
        );
    }
}
