//! The canonical memory record.

use crate::content::MemoryContent;
use crate::id::MemoryId;
use crate::provenance::{Provenance, RetentionPolicy, SourceKind};
use crate::scope::MemoryScope;
use crate::subject::MemorySubject;
use crate::taxonomy::{MemoryStatus, MemoryType};
use crate::temporal::ValidityWindow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The single canonical representation stored by every provider.
///
/// All operational views (profile, conversation, entity, ...) are
/// indexes over this shape; nothing else is authoritative.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub memory_type: MemoryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,

    #[serde(default, skip_serializing_if = "MemoryScope::is_empty")]
    pub scope: MemoryScope,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<MemorySubject>,
    pub content: MemoryContent,

    /// Caller- or classifier-assigned certainty in `[0, 1]`.
    pub confidence: f32,
    /// Retrieval priority hint in `[0, 1]`, independent of certainty.
    pub importance: f32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<DateTime<Utc>>,

    pub status: MemoryStatus,
    /// Monotonic revision; bumped by every update and supersession.
    pub version: u64,
    /// Predecessor this record replaced, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<MemoryId>,

    pub provenance: Provenance,
    pub retention: RetentionPolicy,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemoryRecord {
    /// Builds a new active record with generated identity and stamps.
    ///
    /// Confidence/importance default to neutral midpoints so callers who
    /// do not model these still get sane ranking behavior.
    pub fn new(memory_type: MemoryType, content: MemoryContent) -> Self {
        let now = Utc::now();
        Self {
            id: MemoryId::generate(),
            memory_type,
            subtype: None,
            scope: MemoryScope::default(),
            subject: None,
            content,
            confidence: 0.5,
            importance: 0.5,
            valid_from: None,
            valid_to: None,
            status: MemoryStatus::initial(),
            version: 1,
            supersedes: None,
            provenance: Provenance::now(SourceKind::Agent),
            retention: RetentionPolicy::standard(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Clamps `confidence` into `[0, 1]` and returns the record.
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Clamps `importance` into `[0, 1]` and returns the record.
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// Sets the scope partition.
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = scope;
        self
    }

    /// Sets the subject.
    pub fn with_subject(mut self, subject: MemorySubject) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Sets the subtype label (e.g. "preference", "commitment").
    pub fn with_subtype(mut self, subtype: impl Into<String>) -> Self {
        self.subtype = Some(subtype.into());
        self
    }

    /// Sets provenance.
    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = provenance;
        self
    }

    /// Sets retention policy.
    pub fn with_retention(mut self, retention: RetentionPolicy) -> Self {
        self.retention = retention;
        self
    }

    /// Sets the validity window.
    pub fn with_validity(mut self, window: ValidityWindow) -> Self {
        self.valid_from = window.from;
        self.valid_to = window.to;
        self
    }

    /// The record's validity as a window struct.
    pub fn validity(&self) -> ValidityWindow {
        ValidityWindow {
            from: self.valid_from,
            to: self.valid_to,
        }
    }

    /// True when the fact holds at `at` and the record is retrievable.
    pub fn is_valid_at(&self, at: DateTime<Utc>) -> bool {
        self.status.is_retrievable() && self.validity().contains(at)
    }

    /// True when retention has lapsed at `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.retention.is_expired(now)
    }

    /// Produces the successor of this record for an update-in-place.
    ///
    /// The successor gets fresh identity, inherits scope/type/provenance
    /// lineage, and points back at this record via `supersedes`; the
    /// original is left untouched so history is never destroyed.
    pub fn derive_successor(&self, content: MemoryContent) -> Self {
        let mut next = Self::new(self.memory_type, content);
        next.scope = self.scope.clone();
        next.subtype = self.subtype.clone();
        next.subject = self.subject.clone();
        next.confidence = self.confidence;
        next.importance = self.importance;
        next.retention = self.retention;
        next.supersedes = Some(self.id);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn sample() -> MemoryRecord {
        MemoryRecord::new(
            MemoryType::Semantic,
            MemoryContent::from_text("Project Atlas uses PostgreSQL"),
        )
    }

    #[test]
    fn new_records_start_active_at_version_one() {
        let r = sample();
        assert_eq!(r.status, MemoryStatus::Active);
        assert_eq!(r.version, 1);
        assert_eq!(r.supersedes, None);
        assert_eq!(r.created_at, r.updated_at);
        assert_eq!(r.confidence, 0.5);
    }

    #[test]
    fn builder_clamps_scores() {
        let r = sample().with_confidence(1.7).with_importance(-3.0);
        assert_eq!(r.confidence, 1.0);
        assert_eq!(r.importance, 0.0);
    }

    #[test]
    fn validity_respects_status_and_window() {
        let now = Utc::now();
        let mut r = sample();
        assert!(r.is_valid_at(now));

        r.valid_to = Some(now - Duration::hours(1));
        assert!(!r.is_valid_at(now));

        r.valid_to = None;
        r.status = MemoryStatus::Deleted;
        assert!(!r.is_valid_at(now));
    }

    #[test]
    fn successors_link_back_without_mutating_origin() {
        let origin = sample().with_subject(MemorySubject::new("atlas"));
        let next = origin.derive_successor(MemoryContent::from_text("Atlas uses MySQL"));

        assert_eq!(next.supersedes, Some(origin.id));
        assert_eq!(next.version, 1, "successor starts its own revision line");
        assert_ne!(next.id, origin.id);
        assert_eq!(next.subject, origin.subject);
        assert_eq!(origin.supersedes, None, "history untouched");
    }

    #[test]
    fn serde_roundtrip_preserves_full_record() {
        let r = sample()
            .with_scope(
                MemoryScope::builder()
                    .tenant("acme")
                    .user("u-1")
                    .build(),
            )
            .with_subtype("fact")
            .with_confidence(0.9)
            .with_retention(RetentionPolicy::permanent());
        let json = serde_json::to_string(&r).expect("serialize");
        let back: MemoryRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, r);
        assert!(json.contains("\"scope\":"), "pinned scope serialized");

        let bare = MemoryRecord::new(MemoryType::Working, MemoryContent::from_text("hi"));
        let bare_json = serde_json::to_string(&bare).expect("serialize");
        assert!(
            !bare_json.contains("\"scope\":"),
            "empty scope omitted to keep payloads small"
        );
    }
}
