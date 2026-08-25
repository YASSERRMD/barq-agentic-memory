//! The plan model: an ordered pipeline of lookup steps.
//!
//! Plans are plain data — inspectable, serializable, and cheap to test
//! — so callers (and humans) can see exactly how a recall ran.

use chrono::{DateTime, Utc};
use memory_domain::MemoryType;
use serde::{Deserialize, Serialize};

/// Which backend a step targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Canonical record store (exact/structured lookups).
    Store,
    /// Vector similarity index.
    Vector,
    /// Working-memory session state.
    Working,
    /// Graph store for relation traversal (phase 11).
    Graph,
}

/// How candidates are found in a step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LookupKind {
    /// Exact subject match on structured fields.
    ExactSubject,
    /// Keyword/text containment over canonical records.
    Keyword,
    /// Embedding similarity search.
    Semantic {
        /// The query text to embed.
        query_text: String,
    },
    /// Graph relations around a subject (phase 11).
    GraphRelations,
}

/// One lookup in the ordered plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalStep {
    /// Backend this step targets.
    pub provider: ProviderKind,
    /// Lookup strategy.
    pub kind: LookupKind,
    /// Memory types the step accepts; empty means all.
    pub memory_types: Vec<MemoryType>,
    /// Temporal snapshot for validity filtering.
    pub valid_at: DateTime<Utc>,
    /// Candidate budget for this step alone.
    pub limit: u32,
}

impl RetrievalStep {
    /// True when the step needs an embedding backend.
    pub fn requires_embeddings(&self) -> bool {
        matches!(self.kind, LookupKind::Semantic { .. })
    }

    /// True when the step needs a graph backend.
    pub fn requires_graph(&self) -> bool {
        self.provider == ProviderKind::Graph
    }
}

/// Ordered steps plus plan-level metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalPlan {
    /// Steps run in order; earlier steps win merge priority.
    pub steps: Vec<RetrievalStep>,
    /// Total candidate budget across all steps.
    pub budget: u32,
}

impl RetrievalPlan {
    /// True when any step needs embeddings.
    pub fn requires_embeddings(&self) -> bool {
        self.steps.iter().any(RetrievalStep::requires_embeddings)
    }

    /// True when any step needs a graph backend.
    pub fn requires_graph(&self) -> bool {
        self.steps.iter().any(RetrievalStep::requires_graph)
    }

    /// Sum of per-step limits, clamped to the plan budget.
    pub fn effective_candidates(&self) -> u32 {
        let sum: u32 = self.steps.iter().map(|s| s.limit).sum();
        sum.min(self.budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn step(provider: ProviderKind, limit: u32) -> RetrievalStep {
        RetrievalStep {
            provider,
            kind: match provider {
                ProviderKind::Vector => LookupKind::Semantic {
                    query_text: "q".into(),
                },
                _ => LookupKind::ExactSubject,
            },
            memory_types: vec![MemoryType::Semantic],
            valid_at: Utc::now(),
            limit,
        }
    }

    #[test]
    fn requirement_flags_reflect_steps() {
        let plan = RetrievalPlan {
            steps: vec![step(ProviderKind::Store, 5), step(ProviderKind::Vector, 10)],
            budget: 12,
        };
        assert!(plan.requires_embeddings());
        assert!(!plan.requires_graph());

        let graph_plan = RetrievalPlan {
            steps: vec![step(ProviderKind::Graph, 4)],
            budget: 4,
        };
        assert!(graph_plan.requires_graph());
        assert!(!graph_plan.requires_embeddings());
    }

    #[test]
    fn effective_budget_clamps_step_sums() {
        let plan = RetrievalPlan {
            steps: vec![step(ProviderKind::Store, 8), step(ProviderKind::Vector, 9)],
            budget: 12,
        };
        assert_eq!(plan.effective_candidates(), 12);
    }

    #[test]
    fn plan_serializes_with_kind_tags() {
        let plan = RetrievalPlan {
            steps: vec![step(ProviderKind::Vector, 3)],
            budget: 3,
        };
        let json = serde_json::to_string(&plan).expect("serialize");
        assert!(json.contains("\"semantic\""), "tagged enum expected");
        let back: RetrievalPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, plan);
    }
}
