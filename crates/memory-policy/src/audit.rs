//! Audit trail events and sinks.

use async_trait::async_trait;
use memory_domain::MemoryResult;
use serde::{Deserialize, Serialize};

/// One auditable governance event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    /// ISO instant of the event.
    pub at: chrono::DateTime<chrono::Utc>,
    /// Who attempted access ("user:u-1", "agent:researcher").
    pub principal: String,
    /// What they tried to do.
    pub action: AuditAction,
    /// Which record (when known).
    pub record_id: Option<memory_domain::MemoryId>,
    /// Whether the engine allowed it.
    pub allowed: bool,
    /// Why (denial reason or rule name).
    pub detail: String,
}

/// Governance-relevant actions worth auditing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Read,
    Write,
    Delete,
    Export,
}

/// Receives audit events. Implementations forward to SIEM/logging;
/// failures must never block the governed operation.
#[async_trait]
pub trait Auditor: Send + Sync {
    fn name(&self) -> &str;
    async fn record(&self, event: AuditEvent) -> MemoryResult<()>;
}

/// In-memory auditor for tests and local inspection.
#[derive(Default)]
pub struct InMemoryAuditor {
    pub events: std::sync::Mutex<Vec<AuditEvent>>,
}

impl InMemoryAuditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.events.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl Auditor for InMemoryAuditor {
    fn name(&self) -> &str {
        "in-memory"
    }
    async fn record(&self, event: AuditEvent) -> MemoryResult<()> {
        self.events.lock().expect("poisoned").push(event);
        Ok(())
    }
}

