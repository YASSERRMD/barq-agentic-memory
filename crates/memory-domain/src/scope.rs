//! Multi-tenant scoping model.
//!
//! A [`MemoryScope`] names the partition a memory belongs to. Every
//! dimension is optional; when a query sets a dimension it acts as an
//! equality filter, unset dimensions are wildcards.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Partition keys attached to every memory record and query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl MemoryScope {
    /// Creates a builder for ergonomic scope construction.
    pub fn builder() -> MemoryScopeBuilder {
        MemoryScopeBuilder::default()
    }

    /// True if no dimension is pinned.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Number of pinned dimensions.
    pub fn specificity(&self) -> usize {
        [
            self.tenant_id.as_ref(),
            self.organization_id.as_ref(),
            self.workspace_id.as_ref(),
            self.user_id.as_ref(),
            self.agent_id.as_ref(),
            self.session_id.as_ref(),
            self.task_id.as_ref(),
        ]
        .iter()
        .filter(|d| d.is_some())
        .count()
    }

    /// True when `candidate` belongs inside this scope: every dimension
    /// pinned here must match exactly; dimensions left unset here are
    /// wildcards.
    pub fn contains(&self, candidate: &MemoryScope) -> bool {
        dim_matches(self.tenant_id.as_deref(), candidate.tenant_id.as_deref())
            && dim_matches(
                self.organization_id.as_deref(),
                candidate.organization_id.as_deref(),
            )
            && dim_matches(
                self.workspace_id.as_deref(),
                candidate.workspace_id.as_deref(),
            )
            && dim_matches(self.user_id.as_deref(), candidate.user_id.as_deref())
            && dim_matches(self.agent_id.as_deref(), candidate.agent_id.as_deref())
            && dim_matches(self.session_id.as_deref(), candidate.session_id.as_deref())
            && dim_matches(self.task_id.as_deref(), candidate.task_id.as_deref())
    }

    /// Narrows this scope with the pinned dimensions of `other`.
    ///
    /// Conflicting pins return [`None`] rather than silently picking a
    /// winner, because a contradictory scope would otherwise leak or hide
    /// memories unpredictably.
    pub fn intersect(&self, other: &MemoryScope) -> Option<MemoryScope> {
        let merged = MemoryScope {
            tenant_id: reconcile(self.tenant_id.clone(), other.tenant_id.clone())?,
            organization_id: reconcile(
                self.organization_id.clone(),
                other.organization_id.clone(),
            )?,
            workspace_id: reconcile(self.workspace_id.clone(), other.workspace_id.clone())?,
            user_id: reconcile(self.user_id.clone(), other.user_id.clone())?,
            agent_id: reconcile(self.agent_id.clone(), other.agent_id.clone())?,
            session_id: reconcile(self.session_id.clone(), other.session_id.clone())?,
            task_id: reconcile(self.task_id.clone(), other.task_id.clone())?,
        };
        Some(merged)
    }
}

fn dim_matches(expected: Option<&str>, actual: Option<&str>) -> bool {
    match expected {
        None => true,
        Some(pin) => actual == Some(pin),
    }
}

fn reconcile(a: Option<String>, b: Option<String>) -> Option<Option<String>> {
    match (a, b) {
        (None, any) | (any, None) => Some(any),
        (x, y) if x == y => Some(x),
        _ => None,
    }
}

impl fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = [
            ("tenant", self.tenant_id.as_deref()),
            ("org", self.organization_id.as_deref()),
            ("workspace", self.workspace_id.as_deref()),
            ("user", self.user_id.as_deref()),
            ("agent", self.agent_id.as_deref()),
            ("session", self.session_id.as_deref()),
            ("task", self.task_id.as_deref()),
        ];
        let pinned: Vec<String> = parts
            .iter()
            .filter_map(|(k, v)| v.map(|v| format!("{k}={v}")))
            .collect();
        write!(f, "scope[{}]", pinned.join(","))
    }
}

/// Fluent builder for [`MemoryScope`].
#[derive(Clone, Debug, Default)]
pub struct MemoryScopeBuilder(MemoryScope);

macro_rules! scope_dim {
    ($method:ident, $field:ident) => {
        #[allow(missing_docs)]
        pub fn $method(mut self, value: impl Into<String>) -> Self {
            self.0.$field = Some(value.into());
            self
        }
    };
}

impl MemoryScopeBuilder {
    scope_dim!(tenant, tenant_id);
    scope_dim!(organization, organization_id);
    scope_dim!(workspace, workspace_id);
    scope_dim!(user, user_id);
    scope_dim!(agent, agent_id);
    scope_dim!(session, session_id);
    scope_dim!(task, task_id);

    /// Finishes construction.
    pub fn build(self) -> MemoryScope {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_scope() -> MemoryScope {
        MemoryScope::builder()
            .tenant("acme")
            .user("u-1")
            .build()
    }

    #[test]
    fn empty_scope_is_universal_wildcard() {
        let wildcard = MemoryScope::default();
        assert!(wildcard.is_empty());
        assert!(wildcard.contains(&record_scope()));
    }

    #[test]
    fn pinned_dimensions_must_match() {
        let query = MemoryScope::builder().tenant("acme").build();
        assert!(query.contains(&record_scope()));

        let other_tenant = MemoryScope::builder().tenant("other").build();
        assert!(!query.contains(&MemoryScope { tenant_id: None, ..other_tenant }));
        assert!(query.contains(&record_scope()), "same tenant passes");
        assert!(!query.contains(&other_tenant));
    }

    #[test]
    fn records_without_pinned_dimension_are_excluded() {
        let query = MemoryScope::builder().user("u-1").build();
        let anonymous = MemoryScope::builder().tenant("acme").build();
        assert!(!query.contains(&anonymous));
    }

    #[test]
    fn intersect_merges_disjoint_and_accepts_equal_pins() {
        let a = MemoryScope::builder().tenant("acme").build();
        let b = MemoryScope::builder().tenant("acme").user("u-1").build();

        let merged = a.intersect(&b).expect("compatible");
        assert_eq!(merged.specificity(), 2);
    }

    #[test]
    fn intersect_rejects_contradictions() {
        let a = MemoryScope::builder().tenant("acme").build();
        let b = MemoryScope::builder().tenant("globex").build();
        assert!(a.intersect(&b).is_none());
    }

    #[test]
    fn display_lists_only_pinned_dimensions() {
        let s = record_scope();
        let rendered = s.to_string();
        assert!(rendered.contains("tenant=acme"));
        assert!(rendered.contains("user=u-1"));
        assert!(!rendered.contains("workspace"));
    }

    #[test]
    fn serde_roundtrip_with_defaults() {
        let s = record_scope();
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(!json.contains("organization"), "unset dims skipped");
        let back: MemoryScope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, s);
    }
}
