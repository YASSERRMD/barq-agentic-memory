//! The dedup decision model.

use memory_domain::MemoryId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What the engine should do with an incoming record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DedupAction {
    /// Nothing matches; store as new.
    Add,
    /// A duplicate exists; keep the original untouched.
    Ignore,
    /// Same fact, newer content; supersede via the update path.
    Merge,
    /// Related but distinct; store both (graph links arrive later).
    Link,
    /// Signals conflict or sit near thresholds; quarantine for review.
    Review,
}

impl fmt::Display for DedupAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            DedupAction::Add => "add",
            DedupAction::Ignore => "ignore",
            DedupAction::Merge => "merge",
            DedupAction::Link => "link",
            DedupAction::Review => "review",
        };
        f.write_str(name)
    }
}

/// Decision plus its evidence trail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DedupDecision {
    pub action: DedupAction,
    /// Existing record the action refers to (Ignore/Merge/Link).
    pub target: Option<MemoryId>,
    /// Human-readable evidence chain for audits.
    pub reason: String,
}

impl DedupDecision {
    pub fn add() -> Self {
        Self {
            action: DedupAction::Add,
            target: None,
            reason: "no matching signals".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_serialize_snake_case() {
        for a in [
            DedupAction::Add,
            DedupAction::Ignore,
            DedupAction::Merge,
            DedupAction::Link,
            DedupAction::Review,
        ] {
            let json = serde_json::to_string(&a).expect("serialize");
            assert_eq!(json, format!("\"{}\"", a));
        }
    }

    #[test]
    fn add_decision_has_no_target() {
        let d = DedupDecision::add();
        assert_eq!(d.action, DedupAction::Add);
        assert!(d.target.is_none());
    }
}
