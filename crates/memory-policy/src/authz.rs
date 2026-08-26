//! Authorization: who may see or write what.

use async_trait::async_trait;
use memory_domain::{MemoryRecord, MemoryResult};

/// The caller attempting access.
#[derive(Clone, Debug, PartialEq)]
pub struct Principal {
    /// Stable identity (user or service id).
    pub id: String,
    /// Coarse roles; authorizers interpret them freely.
    pub roles: Vec<String>,
    /// Scope the principal acts within.
    pub scope: memory_domain::MemoryScope,
}

impl Principal {
    /// A scoped human/service identity with no roles.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            roles: Vec::new(),
            scope: memory_domain::MemoryScope::default(),
        }
    }

    /// Attaches a role.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.roles.push(role.into());
        self
    }

    /// Pins the acting scope.
    pub fn with_scope(mut self, scope: memory_domain::MemoryScope) -> Self {
        self.scope = scope;
        self
    }
}

/// Read/write authorization over records.
///
/// Deny answers must be authoritative: the engine treats `false` as a
/// hard filter — the record never reaches serialization, not even as
/// metadata.
#[async_trait]
pub trait Authorizer: Send + Sync {
    fn name(&self) -> &str;

    /// May `principal` see this record?
    async fn authorize_read(&self, principal: &Principal, record: &MemoryRecord) -> bool;

    /// May `principal` write this record?
    async fn authorize_write(&self, principal: &Principal, record: &MemoryRecord) -> bool;
}

/// Scope-identity authorizer: principals see exactly what their pinned
/// scope dimensions match. Roles can widen nothing; this is the safe
/// default for multi-tenant deployments.
pub struct ScopeAuthorizer;

#[async_trait]
impl Authorizer for ScopeAuthorizer {
    fn name(&self) -> &str {
        "scope"
    }

    async fn authorize_read(&self, principal: &Principal, record: &MemoryRecord) -> bool {
        principal.scope.contains(&record.scope)
    }

    async fn authorize_write(&self, principal: &Principal, record: &MemoryRecord) -> bool {
        // Writes must land inside the principal's own scope; writing
        // into a narrower scope is allowed, escaping it is not.
        principal.scope.contains(&record.scope)
    }
}

/// Result helpers shared by policy checks.
pub type PolicyResult<T> = MemoryResult<T>;

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryContent, MemoryScopeBuilder, MemoryType};

    #[tokio::test]
    async fn tenants_cannot_read_across_partitions() {
        let az = ScopeAuthorizer;
        let acme = Principal::new("u-1").with_role("member");
        let mut acme_principal = acme.clone();
        acme_principal.scope = MemoryScopeBuilder::new().tenant("acme").build();

        let mut foreign = MemoryRecord::new(
            MemoryType::Semantic,
            MemoryContent::from_text("globex secret"),
        );
        foreign.scope = MemoryScopeBuilder::new().tenant("globex").build();

        assert!(!az.authorize_read(&acme_principal, &foreign).await);
    }

    #[tokio::test]
    async fn writes_may_narrow_but_not_escape_scope() {
        let az = ScopeAuthorizer;
        let mut principal = Principal::new("svc-etl");
        principal.scope = MemoryScopeBuilder::new().tenant("acme").build();

        let mut inside = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("x"));
        inside.scope = MemoryScopeBuilder::new().tenant("acme").user("u-9").build();
        let mut outside = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("y"));
        outside.scope = MemoryScopeBuilder::new().tenant("other").build();

        assert!(az.authorize_write(&principal, &inside).await);
        assert!(!az.authorize_write(&principal, &outside).await);
    }

    #[tokio::test]
    async fn wildcard_principals_see_everything() {
        let az = ScopeAuthorizer;
        let admin = Principal::new("root"); // empty scope = wildcard
        let any = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("z"));
        assert!(az.authorize_read(&admin, &any).await);
    }
}
