//! Complete mock-driven suite: every engine subsystem exercised
//! end-to-end with recording providers and injected failures.
//! Zero infrastructure, fully deterministic — this file is the
//! executable specification of the engine's behavior contract.

use memory_core::{EngineParts, MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::{
    MemoryError, MemoryId, MemoryScopeBuilder, MemoryType, RetentionClass, RetentionPolicy,
    SourceKind, config::EngineConfig,
};
use memory_policy::{AuditAction, InMemoryAuditor};
use memory_provider_api::MemoryStoreProvider as _;
use memory_retrieval::RecallRequest;
use memory_testkit::{
    FailPlan, FailureKnobs, MockEmbedder, MockStore, MockVector, MockWorking, StoreCall, VectorCall,
};
use std::sync::Arc;

fn config(mut tweak: impl FnMut(&mut EngineConfig)) -> EngineConfig {
    let mut c = EngineConfig::default();
    tweak(&mut c);
    c
}

fn mock_engine(cfg: EngineConfig) -> (MemoryEngine, Arc<MockStore>, Arc<MockVector>) {
    mock_engine_with(cfg, FailureKnobs::none(), FailureKnobs::none())
}

fn mock_engine_with(
    cfg: EngineConfig,
    store_knobs: FailureKnobs,
    vector_knobs: FailureKnobs,
) -> (MemoryEngine, Arc<MockStore>, Arc<MockVector>) {
    let store = MockStore::shared(store_knobs);
    let vector = MockVector::shared(vector_knobs);
    let embedder = Arc::new(MockEmbedder::new(64));
    let engine = MemoryEngine::from_parts(
        cfg,
        EngineParts {
            store: store.clone(),
            working: MockWorking::shared(FailureKnobs::none()),
            vector: Some(vector.clone()),
            embedder: Some(embedder),
        },
    )
    .expect("engine from parts");
    (engine, store, vector)
}

// ---------------------------------------------------------------- core

#[tokio::test]
async fn six_concepts_full_lifecycle_through_mocks() {
    let (engine, store, vector) = mock_engine(config(|_| {}));

    // remember — hits store + vector.
    let v1 = engine
        .remember(RememberRequest::new(
            MemoryType::Semantic,
            "Atlas uses MySQL",
        ))
        .await
        .unwrap();
    assert!(vector.saw(&VectorCall::Upsert(v1.id)));

    // recall (hybrid) finds it via the vector path.
    let hits = engine
        .recall(&RecallRequest::new("which database atlas").with_budget(5))
        .await
        .unwrap();
    assert!(hits.iter().any(|c| c.record.id == v1.id));

    // search (keyword) via store query.
    assert!(
        !engine
            .search(memory_domain::MemoryQuery::default().with_text("mysql"))
            .await
            .unwrap()
            .is_empty()
    );

    // update superseded the predecessor everywhere.
    let v2 = engine
        .update(UpdateRequest::content(
            v1.id,
            Default::default(),
            "Atlas uses PostgreSQL",
        ))
        .await
        .unwrap();
    assert!(vector.saw(&VectorCall::Delete(v1.id)));
    assert!(vector.saw(&VectorCall::Upsert(v2.id)));

    // history chains both generations.
    let chain = engine.history(v2.id, &Default::default()).await.unwrap();
    assert_eq!(chain.len(), 2);

    // forget tombstones and removes the vector.
    assert!(engine.forget(v2.id, &Default::default()).await.unwrap());
    assert!(vector.saw(&VectorCall::Delete(v2.id)));
    let gone = engine
        .recall_exact(v2.id, &Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(gone.status, memory_domain::MemoryStatus::Deleted);

    // The mock observed the full ordered contract.
    assert!(store.saw(&StoreCall::Put(v1.id)));
    assert!(store.saw(&StoreCall::Query));
    assert!(store.saw(&StoreCall::Update(v1.id)));
}

#[tokio::test]
async fn validation_rejects_before_any_provider_is_touched() {
    let (engine, store, vector) = mock_engine(config(|_| {}));
    let err = engine
        .remember(RememberRequest::new(MemoryType::Semantic, "   "))
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryError::Validation { .. }));
    assert!(store.calls().is_empty(), "store untouched");
    assert!(vector.calls().is_empty(), "vector untouched");
}

#[tokio::test]
async fn scope_isolation_end_to_end() {
    let (engine, store, _) = mock_engine(config(|_| {}));
    let acme = MemoryScopeBuilder::new().tenant("acme").user("u-1").build();

    let secret = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "acme confidential roadmap")
                .with_scope(acme),
        )
        .await
        .unwrap();

    let globex = MemoryScopeBuilder::new().tenant("globex").build();
    assert!(
        engine
            .recall_exact(secret.id, &globex)
            .await
            .unwrap()
            .is_none(),
        "cross-tenant exact read is invisible"
    );
    assert!(
        engine
            .recall(&RecallRequest::new("acme roadmap").with_scope(globex))
            .await
            .unwrap()
            .is_empty(),
        "cross-tenant hybrid recall is empty"
    );
    assert_eq!(store.len(), 1, "record still exists for its owner");
}

// ------------------------------------------------------------- vector

#[tokio::test]
async fn vector_failure_surfaces_remember_errors() {
    // Correctness-first contract: a record that cannot be indexed must
    // not pretend to be recallable.
    let (engine, store, vector) = mock_engine_with(
        config(|_| {}),
        FailureKnobs::none(),
        FailureKnobs {
            put: FailPlan::Always,
            ..FailureKnobs::none()
        },
    );
    let err = engine
        .remember(RememberRequest::new(
            MemoryType::Semantic,
            "will fail indexing",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryError::ProviderUnavailable { .. }));
    assert_eq!(vector.len(), 0, "nothing indexed");
    assert_eq!(
        store.len(),
        1,
        "canonical write landed before the index attempt"
    );
}

#[tokio::test]
async fn health_degrades_but_exact_retrieval_survives_vector_outage() {
    let (engine, _, _) = mock_engine_with(
        config(|_| {}),
        FailureKnobs::none(),
        FailureKnobs {
            query: FailPlan::Always,
            ..FailureKnobs::none()
        },
    );
    let saved = engine
        .remember(RememberRequest::new(
            MemoryType::Semantic,
            "still findable exactly",
        ))
        .await
        .unwrap();

    let health = engine.health().await.unwrap();
    assert_eq!(
        health.vector.as_ref().unwrap().status,
        memory_policy_status_degraded()
    );
    assert!(
        health.exact_retrieval_available(),
        "graceful degradation contract"
    );
    assert!(
        engine
            .recall_exact(saved.id, &Default::default())
            .await
            .unwrap()
            .is_some()
    );
}

fn memory_policy_status_degraded() -> memory_reliability::HealthStatus {
    memory_reliability::HealthStatus::Degraded
}

#[tokio::test]
async fn index_repair_uses_list_ids_and_reconciles() {
    let (engine, store, vector) = mock_engine(config(|_| {}));

    // A record stored without inline indexing (async-mode write).
    let orphan = engine
        .write_unindexed(RememberRequest::new(
            MemoryType::Semantic,
            "missed by the indexer",
        ))
        .await
        .unwrap();
    // A ghost: vector present, canonical row purged.
    let purged = engine
        .remember(RememberRequest::new(MemoryType::Semantic, "will be purged"))
        .await
        .unwrap();
    store.delete(&purged.id, &Default::default()).await.unwrap();

    let (ghosts, reindexed) = engine.repair_vector_index().await.unwrap();
    assert_eq!(ghosts, 1);
    assert_eq!(reindexed, 1);
    assert!(vector.saw(&VectorCall::ListIds));
    assert!(
        !vector.saw(&VectorCall::Delete(orphan.id)),
        "orphans re-index, not deleted"
    );
}

// --------------------------------------------------------------- dedup

#[tokio::test]
async fn dedup_ignores_exact_duplicates_and_returns_the_original() {
    let (engine, _, _) = mock_engine(config(|c| c.dedup_enabled = true));
    let mk = || {
        RememberRequest::new(MemoryType::Semantic, "Customer prefers email")
            .with_subject(memory_domain::MemorySubject::new("cust-1"))
    };
    let first = engine.remember(mk()).await.unwrap();
    let second = engine.remember(mk()).await.unwrap();
    assert_eq!(first.id, second.id, "duplicate collapses to the original");
}

#[tokio::test]
async fn dedup_merges_reworded_same_subject_statements() {
    let (engine, store, _) = mock_engine(config(|c| c.dedup_enabled = true));

    let a = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "atlas standardizes on postgres")
                .with_subject(memory_domain::MemorySubject::new("atlas")),
        )
        .await
        .unwrap();
    let before = store.len();

    let b = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "atlas standardizes on postgres v2")
                .with_subject(memory_domain::MemorySubject::new("atlas")),
        )
        .await
        .unwrap();

    assert_ne!(a.id, b.id, "merge produces a successor, not a passthrough");
    assert_eq!(
        store.len(),
        before + 1,
        "successor stored; predecessor retired in place"
    );
    let retired = engine
        .recall_exact(a.id, &Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, memory_domain::MemoryStatus::Superseded);
}

#[tokio::test]
async fn distinct_facts_never_collide_under_dedup() {
    let (engine, store, _) = mock_engine(config(|c| c.dedup_enabled = true));
    engine
        .remember(RememberRequest::new(
            MemoryType::Semantic,
            "Atlas uses PostgreSQL",
        ))
        .await
        .unwrap();
    engine
        .remember(RememberRequest::new(
            MemoryType::Semantic,
            "Kitchen needs restocking",
        ))
        .await
        .unwrap();
    assert_eq!(store.len(), 2, "unrelated statements both stored");
}

// ------------------------------------------------------------ conflict

#[tokio::test]
async fn negation_supersedes_and_closes_the_predecessor_window() {
    let (engine, _, _) = mock_engine(config(|c| c.conflict_enabled = true));
    let old = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "Atlas runs on MySQL")
                .with_subject(memory_domain::MemorySubject::new("atlas"))
                .from_source(SourceKind::User, "u-1"),
        )
        .await
        .unwrap();

    let correction = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "Atlas no longer uses MySQL")
                .with_subject(memory_domain::MemorySubject::new("atlas"))
                .from_source(SourceKind::User, "u-1"),
        )
        .await
        .unwrap();

    let retired = engine
        .recall_exact(old.id, &Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, memory_domain::MemoryStatus::Superseded);
    assert!(retired.validity().has_ended(chrono::Utc::now()));
    let live = engine
        .recall_exact(correction.id, &Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.status, memory_domain::MemoryStatus::Active);
}

#[tokio::test]
async fn weaker_ambiguous_claims_quarantine_instead_of_guessing() {
    let (engine, _, _) = mock_engine(config(|c| c.conflict_enabled = true));
    engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "Atlas deploys to us-east-1")
                .with_subject(memory_domain::MemorySubject::new("atlas"))
                .from_source(SourceKind::User, "u-1")
                .with_confidence(0.9),
        )
        .await
        .unwrap();

    let weak = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "Atlas deploys to eu-west-1 now")
                .with_subject(memory_domain::MemorySubject::new("atlas"))
                .from_source(SourceKind::Agent, "a-7")
                .with_confidence(0.3),
        )
        .await
        .unwrap();

    assert_eq!(weak.status, memory_domain::MemoryStatus::Quarantined);
}

// ------------------------------------------------------------ working

#[tokio::test]
async fn working_state_snapshot_mutation_survives_the_contract() {
    let (engine, _, _) = mock_engine(config(|_| {}));
    engine
        .set_working_state("sess-1", serde_json::json!({"step": "start"}))
        .await
        .unwrap();

    engine
        .working_push_observation("sess-1", "tests are green")
        .await
        .unwrap();

    let snapshot = engine.working_snapshot("sess-1").await.unwrap().unwrap();
    assert_eq!(
        snapshot.recent_observations,
        vec!["tests are green".to_string()]
    );

    engine.clear_working_state("sess-1").await.unwrap();
    assert!(engine.working_state("sess-1").await.unwrap().is_none());
}

// ---------------------------------------------------------- governance

struct DenyAll;

#[async_trait::async_trait]
impl memory_policy::Authorizer for DenyAll {
    fn name(&self) -> &str {
        "deny-all"
    }
    async fn authorize_read(
        &self,
        _principal: &memory_policy::Principal,
        _record: &memory_domain::MemoryRecord,
    ) -> bool {
        false
    }
    async fn authorize_write(
        &self,
        _principal: &memory_policy::Principal,
        _record: &memory_domain::MemoryRecord,
    ) -> bool {
        false
    }
}

#[tokio::test]
async fn denied_reads_look_like_absence_and_every_attempt_is_audited() {
    let auditor = Arc::new(InMemoryAuditor::new());

    // Governance engine assembled directly with the mocks.
    let store = MockStore::shared(FailureKnobs::none());
    let vector = MockVector::shared(FailureKnobs::none());
    let engine = MemoryEngine::from_parts(
        EngineConfig::default(),
        EngineParts {
            store,
            working: MockWorking::shared(FailureKnobs::none()),
            vector: Some(vector),
            embedder: Some(Arc::new(MockEmbedder::new(64))),
        },
    )
    .expect("engine")
    .with_authorizer(Arc::new(DenyAll))
    .with_auditor(auditor.clone());

    let saved = engine
        .remember(RememberRequest::new(MemoryType::Semantic, "governed fact"))
        .await
        .unwrap();

    let stranger = memory_policy::Principal::new("user:stranger");
    let seen = engine
        .recall_exact_for(&stranger, saved.id, &Default::default())
        .await
        .unwrap();
    assert!(seen.is_none(), "denial masquerades as absence");

    let hits = engine
        .recall_for(&stranger, &RecallRequest::new("governed").with_budget(5))
        .await
        .unwrap();
    assert!(hits.is_empty(), "hybrid recall filters under governance");

    let events = auditor.events.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .any(|e| e.action == AuditAction::Read && !e.allowed),
        "denial is on the audit trail"
    );
}

// ----------------------------------------------------------- lifecycle

#[tokio::test]
async fn retention_sweep_coordinates_deletion_across_all_backends() {
    let graph = Arc::new(memory_graph::InMemoryGraphStore::new());
    let store = MockStore::shared(FailureKnobs::none());
    let vector = MockVector::shared(FailureKnobs::none());
    let engine = MemoryEngine::from_parts(
        EngineConfig::default(),
        EngineParts {
            store: store.clone(),
            working: MockWorking::shared(FailureKnobs::none()),
            vector: Some(vector.clone()),
            embedder: Some(Arc::new(MockEmbedder::new(64))),
        },
    )
    .expect("engine")
    .with_graph(graph.clone());

    let doomed = engine
        .remember(
            RememberRequest::new(MemoryType::Working, "ephemeral scratch").with_retention(
                RetentionPolicy {
                    class: RetentionClass::Ephemeral,
                    expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(10)),
                },
            ),
        )
        .await
        .unwrap();

    let report = engine
        .run_retention_sweep(&Default::default(), chrono::Utc::now())
        .await
        .unwrap();
    assert!(report.purged.contains(&doomed.id));
    assert!(
        engine
            .recall_exact(doomed.id, &Default::default())
            .await
            .unwrap()
            .is_none(),
        "canonical row gone"
    );
    assert!(
        vector.saw(&VectorCall::Delete(doomed.id)),
        "vector entry coordinated away"
    );
}

// ------------------------------------------------ procedures & goals

#[tokio::test]
async fn procedural_lifecycle_and_illegal_transitions() {
    let (engine, _, _) = mock_engine(config(|_| {}));
    let doc = engine
        .publish_procedure("drain then upgrade", "platform", None, Default::default())
        .await
        .unwrap();

    for state in [
        memory_procedural::ProcedureState::Review,
        memory_procedural::ProcedureState::Approved,
        memory_procedural::ProcedureState::Active,
    ] {
        engine.transition_procedure(doc.id, state).await.unwrap();
    }
    assert_eq!(
        engine
            .recall_active_procedures(&Default::default())
            .await
            .unwrap()
            .len(),
        1
    );

    // Illegal: active back to draft.
    assert!(
        engine
            .transition_procedure(doc.id, memory_procedural::ProcedureState::Draft)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn goals_expire_by_derivation_and_complete_cleanly() {
    let (engine, _, _) = mock_engine(config(|_| {}));
    let goal = engine
        .record_goal(
            "File the expense report",
            Some(chrono::Utc::now() - chrono::Duration::days(1)),
            Vec::new(),
            None,
            None,
            Default::default(),
        )
        .await
        .unwrap();

    let open = engine.recall_open_goals(&Default::default()).await.unwrap();
    let (_, effective) = open.iter().find(|(r, _)| r.id == goal.id).unwrap();
    assert_eq!(*effective, memory_prospective::EffectiveGoalState::Expired);

    engine
        .transition_goal(goal.id, memory_prospective::GoalState::Active)
        .await
        .unwrap();
    engine
        .transition_goal(goal.id, memory_prospective::GoalState::Completed)
        .await
        .unwrap();
    assert!(
        engine
            .recall_open_goals(&Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}

// ------------------------------------------------------- reliability

#[tokio::test]
async fn store_outage_surfaces_as_unhealthy_not_panics() {
    let (engine, _, _) = mock_engine_with(
        config(|_| {}),
        FailureKnobs::all_always(),
        FailureKnobs::none(),
    );
    let health = engine.health().await.unwrap();
    assert_eq!(
        health.store.status,
        memory_reliability::HealthStatus::Unhealthy
    );
    assert!(!health.exact_retrieval_available());
}

#[tokio::test]
async fn transient_store_failure_recovers_on_retry() {
    let (engine, _, _) = mock_engine_with(
        config(|_| {}),
        FailureKnobs {
            put: FailPlan::Times(1),
            ..FailureKnobs::none()
        },
        FailureKnobs::none(),
    );

    // First write fails (injected), retry succeeds under the policy.
    let policy = memory_reliability::RetryPolicy::new(
        3,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(2),
    );
    let saved = memory_reliability::with_retry(policy, || {
        engine.remember(RememberRequest::new(MemoryType::Semantic, "resilient fact"))
    })
    .await
    .unwrap();
    assert!(!saved.id.to_string().is_empty());
}

// ------------------------------------------------------- planner/deep

#[tokio::test]
async fn planner_output_is_inspectable_and_deterministic() {
    let (engine, _, _) = mock_engine(config(|_| {}));
    let mut request = RecallRequest::new("What database does Project Atlas use?")
        .with_subject(memory_domain::MemorySubject::new("atlas"));
    // Pin the temporal snapshot: wall-clock otherwise leaks between calls.
    request.valid_at = Some(chrono::Utc::now());

    let plan_a = engine.plan_recall(&request).unwrap();
    let plan_b = engine.plan_recall(&request).unwrap();
    assert_eq!(plan_a, plan_b);
    assert!(
        plan_a.steps.len() >= 2,
        "subject-pinned: exact first, vector fallback"
    );
}

#[tokio::test]
async fn mock_embedder_overrides_drive_ranking_deterministically() {
    // Same vector for two texts: recall order falls to the deterministic
    // tie-break, and the same input always yields the same order.
    let embedder = Arc::new(MockEmbedder::new(64));
    embedder.make_identical("twin fact alpha", "twin fact beta");

    let store = MockStore::shared(FailureKnobs::none());
    let vector = MockVector::shared(FailureKnobs::none());
    let engine = MemoryEngine::from_parts(
        EngineConfig::default(),
        EngineParts {
            store,
            working: MockWorking::shared(FailureKnobs::none()),
            vector: Some(vector),
            embedder: Some(embedder.clone()),
        },
    )
    .expect("engine");

    let a = engine
        .remember(RememberRequest::new(
            MemoryType::Semantic,
            "twin fact alpha",
        ))
        .await
        .unwrap();
    let b = engine
        .remember(RememberRequest::new(MemoryType::Semantic, "twin fact beta"))
        .await
        .unwrap();

    for _ in 0..5 {
        let hits = engine
            .recall(&RecallRequest::new("twin fact").with_budget(2))
            .await
            .unwrap();
        let ids: Vec<MemoryId> = hits.iter().map(|c| c.record.id).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids, vec![a.id, b.id], "deterministic order across runs");
    }
    // The mock observed every embed call.
    let calls = embedder.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|t| t.contains("twin fact alpha")));
}
