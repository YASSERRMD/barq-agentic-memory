//! Procedure lifecycle over canonical records.

use chrono::{DateTime, Utc};
use memory_domain::{MemoryError, MemoryRecord, MemoryResult, MemoryType};
use serde::{Deserialize, Serialize};

/// Approval lifecycle states, blueprint-ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureState {
    Draft,
    Review,
    Approved,
    Active,
    Deprecated,
    Revoked,
}

impl ProcedureState {
    /// All states in canonical order.
    pub const ALL: [ProcedureState; 6] = [
        ProcedureState::Draft,
        ProcedureState::Review,
        ProcedureState::Approved,
        ProcedureState::Active,
        ProcedureState::Deprecated,
        ProcedureState::Revoked,
    ];

    fn can_transition_to(&self, next: ProcedureState) -> bool {
        use ProcedureState::*;
        matches!(
            (self, next),
            (Draft, Review)
                | (Review, Approved)
                | (Review, Draft)
                | (Approved, Active)
                | (Approved, Revoked)
                | (Active, Deprecated)
                | (Deprecated, Revoked)
                | (Revoked, Draft) // rework after revocation starts fresh
        )
    }

    /// True when procedures in this state are retrievable by default.
    pub fn is_operative(&self) -> bool {
        matches!(self, ProcedureState::Active)
    }
}

/// Structured metadata stored in the record's `content.structured`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcedureMetadata {
    pub state: ProcedureState,
    /// Owner team or agent responsible for the document.
    pub owner: String,
    /// Compatibility tag ("agent-v2", "k8s-1.29", ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_to: Option<DateTime<Utc>>,
}

/// Typed view over a procedural canonical record.
#[derive(Clone, Debug)]
pub struct ProcedureView<'a> {
    record: &'a MemoryRecord,
    metadata: ProcedureMetadata,
}

impl<'a> ProcedureView<'a> {
    /// Interprets a record as a procedure when its structure allows.
    pub fn from_record(record: &'a MemoryRecord) -> Option<ProcedureView<'a>> {
        if record.memory_type != MemoryType::Procedural {
            return None;
        }
        let metadata: ProcedureMetadata =
            serde_json::from_value(record.content.structured.clone()?).ok()?;
        Some(Self { record, metadata })
    }

    /// Wraps raw metadata into a record's structured content.
    pub fn into_content(metadata: &ProcedureMetadata) -> serde_json::Value {
        serde_json::to_value(metadata).expect("serializable")
    }

    pub fn state(&self) -> ProcedureState {
        self.metadata.state
    }

    pub fn owner(&self) -> &str {
        &self.metadata.owner
    }

    pub fn is_currently_effective(&self) -> bool {
        let now = Utc::now();
        self.metadata.effective_from.is_none_or(|f| f <= now)
            && self.metadata.effective_to.is_none_or(|t| t >= now)
    }

    /// The underlying canonical record.
    pub fn record(&self) -> &MemoryRecord {
        self.record
    }
}

/// Validates a lifecycle transition for an existing procedure record.
///
/// Returns nothing on success; errors name the illegal transition so
/// callers see exactly which edge was rejected.
pub fn validate_transition(current: ProcedureState, next: ProcedureState) -> MemoryResult<()> {
    if current.can_transition_to(next) {
        Ok(())
    } else {
        Err(MemoryError::validation(
            "procedure_state",
            format!("illegal transition {current:?} -> {next:?}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::MemoryContent;

    fn procedure_record(state: ProcedureState) -> MemoryRecord {
        let meta = ProcedureMetadata {
            state,
            owner: "platform".into(),
            compatibility: Some("k8s-1.29".into()),
            effective_from: None,
            effective_to: None,
        };
        MemoryRecord::new(
            MemoryType::Procedural,
            MemoryContent::from_text("Rolling restart procedure")
                .with_structured(ProcedureView::into_content(&meta)),
        )
    }

    #[test]
    fn views_parse_only_procedural_records() {
        let r = procedure_record(ProcedureState::Draft);
        let v = ProcedureView::from_record(&r).expect("view");
        assert_eq!(v.state(), ProcedureState::Draft);
        assert_eq!(v.owner(), "platform");

        let mut semantic = r.clone();
        semantic.memory_type = MemoryType::Semantic;
        assert!(ProcedureView::from_record(&semantic).is_none());

        let mut no_meta = r;
        no_meta.content.structured = None;
        assert!(ProcedureView::from_record(&no_meta).is_none());
    }

    #[test]
    fn happy_path_is_draft_to_active() {
        validate_transition(ProcedureState::Draft, ProcedureState::Review).expect("legal");
        validate_transition(ProcedureState::Review, ProcedureState::Approved).expect("legal");
        validate_transition(ProcedureState::Approved, ProcedureState::Active).expect("legal");
        validate_transition(ProcedureState::Active, ProcedureState::Deprecated).expect("legal");
        validate_transition(ProcedureState::Deprecated, ProcedureState::Revoked).expect("legal");
    }

    #[test]
    fn skips_are_illegal_and_revocation_recovers_via_draft() {
        assert!(validate_transition(ProcedureState::Draft, ProcedureState::Active).is_err());
        assert!(validate_transition(ProcedureState::Active, ProcedureState::Draft).is_err());
        validate_transition(ProcedureState::Approved, ProcedureState::Revoked)
            .expect("revoke pre-activation");
        validate_transition(ProcedureState::Revoked, ProcedureState::Draft).expect("rework");
    }

    #[test]
    fn only_active_procedures_are_operative() {
        assert!(ProcedureState::Active.is_operative());
        for s in [
            ProcedureState::Draft,
            ProcedureState::Review,
            ProcedureState::Approved,
            ProcedureState::Deprecated,
            ProcedureState::Revoked,
        ] {
            assert!(!s.is_operative());
        }
    }

    #[test]
    fn effectiveness_window_respected() {
        let meta = ProcedureMetadata {
            state: ProcedureState::Active,
            owner: "platform".into(),
            compatibility: None,
            effective_from: Some(Utc::now() - chrono::Duration::days(1)),
            effective_to: Some(Utc::now() + chrono::Duration::days(1)),
        };
        let mut r = MemoryRecord::new(
            MemoryType::Procedural,
            MemoryContent::from_text("x").with_structured(ProcedureView::into_content(&meta)),
        );
        let v = ProcedureView::from_record(&r).expect("view");
        assert!(v.is_currently_effective());

        if let Some(structured) = &mut r.content.structured {
            structured["effective_to"] =
                serde_json::to_value(Utc::now() - chrono::Duration::days(1)).unwrap();
        }
        let expired = ProcedureView::from_record(&r).expect("view");
        assert!(!expired.is_currently_effective());
    }
}
