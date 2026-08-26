//! Sensitivity classification and audit events.

use async_trait::async_trait;
use memory_domain::{MemoryRecord, MemoryType};
use serde::{Deserialize, Serialize};

/// Sensitivity tiers, lowest to highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Internal,
    Confidential,
    Restricted,
}

impl Sensitivity {
    /// The maximum tier a principal may see; comparisons are `<=`.
    pub fn permits(&self, clearance: &Sensitivity) -> bool {
        clearance >= self
    }
}

/// Assigns a sensitivity tier to a record.
#[async_trait]
pub trait DataClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(&self, record: &MemoryRecord) -> Sensitivity;
}

/// Rule-based tier assignment.
///
/// Defaults are conservative: anything mentioning credentials/keys/PII
/// cues lands at CONFIDENTIAL or above; everything else INTERNAL.
pub struct SensitivityClassifier {
    pub restricted_terms: Vec<String>,
    pub confidential_terms: Vec<String>,
}

impl Default for SensitivityClassifier {
    fn default() -> Self {
        Self {
            restricted_terms: ["secret", "password", "private key", "seed phrase"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            confidential_terms: ["credential", "token", "api key", "ssn", "salary"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

#[async_trait]
impl DataClassifier for SensitivityClassifier {
    fn name(&self) -> &str {
        "rule-based"
    }

    async fn classify(&self, record: &MemoryRecord) -> Sensitivity {
        let lower = record.content.text.to_lowercase();
        if self.restricted_terms.iter().any(|t| lower.contains(t)) {
            return Sensitivity::Restricted;
        }
        if self.confidential_terms.iter().any(|t| lower.contains(t)) {
            return Sensitivity::Confidential;
        }
        // Working memory mirrors active sessions; treat as internal.
        match record.memory_type {
            MemoryType::Working => Sensitivity::Internal,
            _ => Sensitivity::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditAction, AuditEvent, Auditor as _, InMemoryAuditor};
    use memory_domain::MemoryContent;

    #[tokio::test]
    async fn sensitivity_orders_tiers() {
        assert!(Sensitivity::Public.permits(&Sensitivity::Restricted));
        assert!(!Sensitivity::Restricted.permits(&Sensitivity::Public));
    }

    #[tokio::test]
    async fn classifier_escalates_secret_material() {
        let c = SensitivityClassifier::default();
        let mut r = MemoryRecord::new(
            MemoryType::Semantic,
            MemoryContent::from_text("the api key is stored in vault"),
        );
        assert_eq!(c.classify(&r).await, Sensitivity::Confidential);

        r.content = MemoryContent::from_text("shared password with the intern");
        assert_eq!(c.classify(&r).await, Sensitivity::Restricted);
    }

    #[tokio::test]
    async fn auditor_collects_events() {
        let a = InMemoryAuditor::new();
        a.record(AuditEvent {
            at: chrono::Utc::now(),
            principal: "user:u-1".into(),
            action: AuditAction::Read,
            record_id: None,
            allowed: false,
            detail: "scope mismatch".into(),
        })
        .await
        .expect("record");
        assert_eq!(a.len(), 1);
    }
}
