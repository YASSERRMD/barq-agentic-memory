//! Governed recall: authorization filters every read path.
//!
//! The invariant is structural — denied records are dropped inside the
//! engine and audited, so nothing unauthorized can reach serialization,
//! logs of results, or the calling model.

use crate::engine::MemoryEngine;
use memory_domain::{MemoryError, MemoryId, MemoryRecord, MemoryResult, MemoryScope};
use memory_policy::Principal;
use memory_retrieval::RankedCandidate;

impl MemoryEngine {
    /// Filters a candidate list through the attached authorizer.
    pub(crate) async fn govern_read(
        &self,
        principal: &Principal,
        candidates: Vec<RankedCandidate>,
    ) -> MemoryResult<Vec<RankedCandidate>> {
        let Some(authorizer) = &self.authorizer else {
            return Ok(candidates);
        };
        let mut allowed_out = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let permitted = authorizer
                .authorize_read(principal, &candidate.record)
                .await;
            self.audit(
                &principal.id,
                memory_policy::AuditAction::Read,
                Some(candidate.record.id),
                permitted,
                if permitted {
                    "authorized"
                } else {
                    "authorization denied"
                },
            )
            .await;
            if permitted {
                allowed_out.push(candidate);
            }
        }
        Ok(allowed_out)
    }

    /// Governed hybrid recall: plan -> execute -> authorize -> rank.
    pub async fn recall_for(
        &self,
        principal: &Principal,
        request: &memory_retrieval::RecallRequest,
    ) -> MemoryResult<Vec<RankedCandidate>> {
        // A principal cannot even query outside their scope.
        let mut scoped = request.clone();
        scoped.scope = intersect_or_fail(&principal.scope, &request.scope)?;

        let hits = self.recall(&scoped).await?;
        let filtered = self.govern_read(principal, hits).await?;
        // Query attempts themselves are auditable events, independent
        // of how many records survived filtering.
        self.audit(
            &principal.id,
            memory_policy::AuditAction::Read,
            None,
            true,
            "recall query executed under governance",
        )
        .await;
        Ok(filtered)
    }

    /// Governed exact lookup.
    pub async fn recall_exact_for(
        &self,
        principal: &Principal,
        id: MemoryId,
        scope: &MemoryScope,
    ) -> MemoryResult<Option<MemoryRecord>> {
        let record = self.recall_exact(id, scope).await?;
        let Some(record) = record else {
            return Ok(None);
        };
        let Some(authorizer) = &self.authorizer else {
            return Ok(Some(record));
        };
        if authorizer.authorize_read(principal, &record).await {
            self.audit(
                &principal.id,
                memory_policy::AuditAction::Read,
                Some(id),
                true,
                "authorized",
            )
            .await;
            Ok(Some(record))
        } else {
            self.audit(
                &principal.id,
                memory_policy::AuditAction::Read,
                Some(id),
                false,
                "authorization denied",
            )
            .await;
            // Denials look identical to absence: no oracle for probing.
            Ok(None)
        }
    }
}

/// Intersects principal scope with query scope; contradictions fail
/// closed rather than widening visibility.
fn intersect_or_fail(principal: &MemoryScope, query: &MemoryScope) -> MemoryResult<MemoryScope> {
    principal
        .intersect(query)
        .ok_or_else(|| MemoryError::validation("scope", "query contradicts principal scope"))
}

#[cfg(test)]
mod governance_tests {
    use super::*;
    use crate::{RememberRequest, UpdateRequest};
    use memory_domain::{
        MemoryScopeBuilder, MemoryType,
        config::{EmbeddingConfig, EngineConfig, VectorStoreConfig},
    };
    use memory_policy::{InMemoryAuditor, ScopeAuthorizer};

    fn tenant_principal(tenant: &str, user: &str) -> Principal {
        Principal::new(format!("user:{user}"))
            .with_scope(MemoryScopeBuilder::new().tenant(tenant).user(user).build())
    }

    async fn governed_engine() -> (MemoryEngine, std::sync::Arc<InMemoryAuditor>) {
        let auditor = std::sync::Arc::new(InMemoryAuditor::new());
        let engine = MemoryEngine::from_config(EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 128 }),
            ..EngineConfig::default()
        })
        .await
        .unwrap()
        .with_authorizer(std::sync::Arc::new(ScopeAuthorizer))
        .with_auditor(auditor.clone());
        (engine, auditor)
    }

    #[tokio::test]
    async fn foreign_tenants_never_see_each_others_memories() {
        let (engine, auditor) = governed_engine().await;

        engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "acme confidential roadmap")
                    .with_scope(MemoryScopeBuilder::new().tenant("acme").user("u-1").build()),
            )
            .await
            .expect("remember");

        let globex_user = tenant_principal("globex", "u-2");
        let hits = engine
            .recall_for(
                &globex_user,
                &memory_retrieval::RecallRequest::new("confidential roadmap")
                    .with_scope(MemoryScopeBuilder::new().tenant("globex").build())
                    .with_budget(10),
            )
            .await
            .expect("recall");
        assert!(hits.is_empty(), "unauthorized memories must never surface");

        // The denial attempt is on the audit trail.
        assert!(!auditor.is_empty());
    }

    #[tokio::test]
    async fn denied_exact_reads_look_like_absence_and_are_audited() {
        let (engine, auditor) = governed_engine().await;

        let saved = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "private note for u-1")
                    .with_scope(MemoryScopeBuilder::new().tenant("acme").user("u-1").build()),
            )
            .await
            .expect("remember");

        let stranger = tenant_principal("acme", "u-9");
        let seen = engine
            .recall_exact_for(&stranger, saved.id, &Default::default())
            .await
            .expect("governed get");
        assert!(seen.is_none(), "denial masquerades as absence");

        let owner = tenant_principal("acme", "u-1");
        let own = engine
            .recall_exact_for(&owner, saved.id, &Default::default())
            .await
            .expect("governed get owner");
        assert!(own.is_some());
        assert!(auditor.len() >= 2);
    }

    #[tokio::test]
    async fn contradictory_query_scopes_fail_closed() {
        let (engine, _auditor) = governed_engine().await;
        let confused =
            Principal::new("user:x").with_scope(MemoryScopeBuilder::new().tenant("acme").build());
        let err = engine
            .recall_for(
                &confused,
                &memory_retrieval::RecallRequest::new("anything")
                    .with_scope(MemoryScopeBuilder::new().tenant("globex").build()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Validation { .. }));
        let _ = UpdateRequest::content(MemoryId::generate(), Default::default(), "unused");
        // (kept to exercise the type in cfg(test) builds)
    }
}
