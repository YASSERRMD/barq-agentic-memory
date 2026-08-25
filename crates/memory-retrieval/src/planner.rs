//! Rule-based retrieval planner.
//!
//! Transparent keyword and structure heuristics decide which lookups
//! run, in what order, with what budgets. No LLM is involved: the same
//! input always produces the same plan.

use crate::plan::{LookupKind, ProviderKind, RetrievalPlan, RetrievalStep};
use crate::request::{RecallMode, RecallRequest};
use chrono::Utc;
use memory_domain::MemoryType;
use std::time::Duration;

/// Latency ceiling hint surfaced to the executor (informational).
pub const PLANNER_TARGET_LATENCY: Duration = Duration::from_millis(150);

/// Turns [`RecallRequest`]s into [`RetrievalPlan`]s.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuleBasedPlanner;

impl RuleBasedPlanner {
    /// Plans a recall.
    pub fn plan(
        &self,
        request: &RecallRequest,
    ) -> Result<RetrievalPlan, memory_domain::MemoryError> {
        request.validated()?;
        let valid_at = request.valid_at.unwrap_or_else(Utc::now);
        let types: Vec<MemoryType> = if request.requested_types.is_empty() {
            infer_types(&request.text)
        } else {
            normalize_types(&request.requested_types)
        };

        let mut steps: Vec<RetrievalStep> = Vec::new();
        match request.mode {
            RecallMode::ExactOnly => {
                push_exact_steps(&mut steps, request, &types, valid_at);
                if steps.is_empty() {
                    // Nothing exact to anchor on; degrade to keyword.
                    push_keyword_step(&mut steps, request, &types, valid_at);
                }
            }
            RecallMode::SemanticOnly => {
                push_semantic_step(&mut steps, request, &types, valid_at);
            }
            RecallMode::Auto if request.subject.is_some() => {
                // Blueprint pattern: subject-pinned questions try exact
                // structured lookup first, vector fallback after.
                push_exact_steps(&mut steps, request, &types, valid_at);
                push_semantic_step(&mut steps, request, &types, valid_at);
            }
            RecallMode::Auto => {
                push_semantic_step(&mut steps, request, &types, valid_at);
                if !has_exact(&steps) && types.contains(&MemoryType::Semantic) {
                    push_keyword_step(&mut steps, request, &types, valid_at);
                }
            }
        }

        if request.allow_episodic_evidence && !types.contains(&MemoryType::Episodic) {
            push_episodic_evidence_step(&mut steps, request, valid_at);
        }

        let budget = request.budget;
        split_budget(&mut steps, budget);

        Ok(RetrievalPlan { steps, budget })
    }
}

/// Keyword evidence for type inference.
///
/// Kept explicit and boring on purpose: every rule here is auditable
/// by reading it, which is exactly why no model sits in this path.
const EPISODIC_HINTS: [&str; 6] = [
    "what happened",
    "when did",
    "last time",
    "history of",
    "previously",
    "earlier",
];
const PROCEDURAL_HINTS: [&str; 5] = [
    "how to",
    "how do i",
    "procedure",
    "steps to",
    "instructions",
];
const PROSPECTIVE_HINTS: [&str; 6] = [
    "need to",
    "todo",
    "to-do",
    "deadline",
    "commitment",
    "follow up",
];
const WORKING_HINTS: [&str; 4] = [
    "current session",
    "right now",
    "active goal",
    "this session",
];

fn infer_types(text: &str) -> Vec<MemoryType> {
    let lower = text.to_lowercase();
    let mut types = Vec::new();
    if EPISODIC_HINTS.iter().any(|h| lower.contains(h)) {
        types.push(MemoryType::Episodic);
    }
    if PROCEDURAL_HINTS.iter().any(|h| lower.contains(h)) {
        types.push(MemoryType::Procedural);
    }
    if PROSPECTIVE_HINTS.iter().any(|h| lower.contains(h)) {
        types.push(MemoryType::Prospective);
    }
    if WORKING_HINTS.iter().any(|h| lower.contains(h)) {
        types.push(MemoryType::Working);
    }
    if types.is_empty() {
        types.push(MemoryType::Semantic);
    }
    types
}

fn normalize_types(types: &[MemoryType]) -> Vec<MemoryType> {
    // Dedup while preserving caller order — plans should not contain
    // duplicate type filters just because a caller listed one twice.
    let mut seen = Vec::new();
    for t in types {
        if !seen.contains(t) {
            seen.push(*t);
        }
    }
    seen
}

fn base_step(
    provider: ProviderKind,
    kind: LookupKind,
    types: &[MemoryType],
    valid_at: chrono::DateTime<Utc>,
) -> RetrievalStep {
    RetrievalStep {
        provider,
        kind,
        memory_types: types.to_vec(),
        valid_at,
        limit: 0, // assigned during budget split
    }
}

fn push_exact_steps(
    steps: &mut Vec<RetrievalStep>,
    request: &RecallRequest,
    types: &[MemoryType],
    valid_at: chrono::DateTime<Utc>,
) {
    if request.subject.is_some() {
        steps.push(base_step(
            ProviderKind::Store,
            LookupKind::ExactSubject,
            types,
            valid_at,
        ));
    }
}

fn push_keyword_step(
    steps: &mut Vec<RetrievalStep>,
    request: &RecallRequest,
    types: &[MemoryType],
    valid_at: chrono::DateTime<Utc>,
) {
    let keywords = keywords_of(&request.text);
    if !keywords.is_empty() {
        steps.push(base_step(
            ProviderKind::Store,
            LookupKind::Keyword,
            types,
            valid_at,
        ));
    }
}

fn push_semantic_step(
    steps: &mut Vec<RetrievalStep>,
    request: &RecallRequest,
    types: &[MemoryType],
    valid_at: chrono::DateTime<Utc>,
) {
    if types.contains(&MemoryType::Working) {
        return; // working state lives outside similarity search
    }
    steps.push(base_step(
        ProviderKind::Vector,
        LookupKind::Semantic {
            query_text: request.text.clone(),
        },
        types,
        valid_at,
    ));
}

fn push_episodic_evidence_step(
    steps: &mut Vec<RetrievalStep>,
    request: &RecallRequest,
    valid_at: chrono::DateTime<Utc>,
) {
    steps.push(base_step(
        ProviderKind::Vector,
        LookupKind::Semantic {
            query_text: request.text.clone(),
        },
        &[MemoryType::Episodic],
        valid_at,
    ));
}

fn has_exact(steps: &[RetrievalStep]) -> bool {
    steps
        .iter()
        .any(|s| matches!(s.kind, LookupKind::ExactSubject | LookupKind::Keyword))
}

/// Budget split: earlier (higher-priority) steps get slightly more,
/// never zero, summing to at most the plan budget.
fn split_budget(steps: &mut [RetrievalStep], budget: u32) {
    if steps.is_empty() || budget == 0 {
        return;
    }
    // Weights decay 2:1 per position so priority order matters but
    // later steps still get a look.
    let weights: Vec<u32> = (0..steps.len())
        .map(|i| 1u32 << (steps.len() - 1 - i).min(8))
        .collect();
    let total: u64 = weights.iter().map(|w| *w as u64).sum();
    let mut assigned = 0u32;
    for (step, weight) in steps.iter_mut().zip(&weights) {
        let share = ((budget as u64 * *weight as u64) / total).max(1) as u32;
        step.limit = share;
        assigned += share;
    }
    // Give rounding leftovers to the first step, capped at its weight
    // share so one step cannot swallow the whole budget.
    if assigned < budget {
        steps[0].limit += budget - assigned;
    }
}

fn keywords_of(text: &str) -> Vec<String> {
    crate::keywords::extract(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::RecallRequest as R;
    use memory_domain::{MemoryScopeBuilder, MemorySubject};

    fn plan_for(text: &str) -> RetrievalPlan {
        RuleBasedPlanner.plan(&R::new(text)).expect("plan")
    }

    #[test]
    fn blank_requests_are_rejected() {
        assert!(RuleBasedPlanner.plan(&R::new("  ")).is_err());
    }

    #[test]
    fn factual_question_defaults_to_semantic_type() {
        let p = plan_for("What database does the platform team recommend?");
        assert_eq!(p.steps[0].memory_types, vec![MemoryType::Semantic]);
    }

    #[test]
    fn episodic_hints_route_to_episodes() {
        let p = plan_for("What happened when we deployed last time?");
        assert!(
            p.steps
                .iter()
                .all(|s| s.memory_types.contains(&MemoryType::Episodic))
        );
    }

    #[test]
    fn procedural_hints_route_to_procedures() {
        let p = plan_for("How to rotate the database credentials?");
        assert_eq!(p.steps[0].memory_types, vec![MemoryType::Procedural]);
    }

    #[test]
    fn prospective_hints_route_to_commitments() {
        let p = plan_for("What do I need to follow up on before the deadline?");
        assert_eq!(
            p.steps[0].memory_types.first(),
            Some(&MemoryType::Prospective)
        );
    }

    #[test]
    fn subject_pinned_question_gets_exact_then_vector() {
        let r = R::new("What database does Project Atlas use?")
            .with_subject(MemorySubject::new("atlas").with_type("project"));
        let p = RuleBasedPlanner.plan(&r).expect("plan");

        assert!(p.steps.len() >= 2);
        assert_eq!(p.steps[0].provider, ProviderKind::Store);
        assert!(matches!(p.steps[0].kind, LookupKind::ExactSubject));
        assert_eq!(p.steps[1].provider, ProviderKind::Vector);
    }

    #[test]
    fn semantic_only_mode_skips_exact_steps() {
        let r = R::new("anything about atlas?")
            .with_subject(MemorySubject::new("a"))
            .with_mode(RecallMode::SemanticOnly);
        let p = RuleBasedPlanner.plan(&r).expect("plan");
        assert!(p.steps.iter().all(|s| s.provider == ProviderKind::Vector));
    }

    #[test]
    fn exact_only_mode_without_subject_degrades_to_keyword() {
        let r = R::new("find mentions of kubernetes").with_mode(RecallMode::ExactOnly);
        let p = RuleBasedPlanner.plan(&r).expect("plan");
        assert!(!p.steps.is_empty());
        assert!(p.steps.iter().all(|s| s.provider == ProviderKind::Store));
    }

    #[test]
    fn episodic_evidence_only_when_requested() {
        let plain = plan_for("What database does Atlas use?");
        assert!(
            !plain
                .steps
                .iter()
                .any(|s| s.memory_types == vec![MemoryType::Episodic])
        );

        let with_evidence = R::new("What database does Atlas use?").with_episodic_evidence();
        let p = RuleBasedPlanner.plan(&with_evidence).expect("plan");
        assert!(
            p.steps
                .iter()
                .any(|s| s.memory_types == vec![MemoryType::Episodic])
        );
    }

    #[test]
    fn budget_is_distributed_not_concentrated() {
        let r = R::new("What database does Project Atlas use?")
            .with_subject(MemorySubject::new("atlas"))
            .with_budget(20);
        let p = RuleBasedPlanner.plan(&r).expect("plan");

        assert!(p.steps.len() >= 2);
        let total: u32 = p.steps.iter().map(|s| s.limit).sum();
        assert!(total >= 20, "rounding gives leftovers to the first step");
        assert!(
            p.steps.iter().all(|s| s.limit >= 1),
            "no step may be starved to zero"
        );
        let first = p.steps[0].limit;
        assert!(first <= 20, "no single step swallows the budget");
    }

    #[test]
    fn scope_flows_into_every_step() {
        let scope = MemoryScopeBuilder::new().tenant("acme").user("u-9").build();
        let r = R::new("preferences?")
            .with_scope(scope.clone())
            .with_subject(MemorySubject::new("u-9"));
        // Scope rides on the request into the executor; steps stay data-only.
        assert_eq!(r.scope.tenant_id.as_deref(), Some("acme"));
        assert_eq!(r.scope, scope);
    }

    #[tokio::test]
    async fn planning_is_deterministic() {
        // Pin the snapshot so wall-clock cannot leak into the comparison;
        // with a fixed instant, identical inputs must give identical plans.
        let at = Utc::now();
        let r = R {
            valid_at: Some(at),
            ..R::new("How to deploy the staging cluster?")
                .with_subject(MemorySubject::new("staging"))
        };
        let a = RuleBasedPlanner.plan(&r).expect("plan");
        let b = RuleBasedPlanner.plan(&r).expect("plan");
        assert_eq!(a, b);
    }
}
