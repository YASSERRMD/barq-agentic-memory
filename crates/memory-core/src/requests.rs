//! Request types for the engine facade.
//!
//! Keeping these separate from [`MemoryRecord`] lets callers express
//! intent without inventing ids, timestamps, or versioning themselves.

use memory_domain::{
    EngineConfig, MemoryContent, MemoryError, MemoryId, MemoryRecord, MemoryResult, MemoryScope,
    MemoryType, Provenance, RetentionPolicy, SourceKind,
};

/// Intent to create a new long-term or short-term memory.
#[derive(Clone, Debug)]
pub struct RememberRequest {
    /// Functional memory kind to classify this record as.
    pub memory_type: MemoryType,
    /// Payload to remember.
    pub content: MemoryContent,
    /// Optional refinement ("preference", "commitment", ...).
    pub subtype: Option<String>,
    /// Scope partition; defaults to the engine's configured scope.
    pub scope: Option<MemoryScope>,
    /// Optional subject anchor.
    pub subject: Option<MemorySubjectBox>,
    /// Certainty in `[0, 1]`; defaults to neutral 0.5.
    pub confidence: f32,
    /// Retrieval priority in `[0, 1]`; defaults to neutral 0.5.
    pub importance: f32,
    /// Retention policy; defaults to standard.
    pub retention: RetentionPolicy,
    /// Origin classification; defaults to [`SourceKind::Agent`].
    pub source: SourceKind,
    /// Specific actor behind the write.
    pub actor_id: Option<String>,
}

/// Subject alias kept import-light for request builders.
pub type MemorySubjectBox = memory_domain::MemorySubject;

impl RememberRequest {
    /// Minimal text memory of a given type.
    pub fn new(memory_type: MemoryType, content: impl Into<String>) -> Self {
        Self {
            memory_type,
            content: MemoryContent::from_text(content),
            subtype: None,
            scope: None,
            subject: None,
            confidence: 0.5,
            importance: 0.5,
            retention: RetentionPolicy::standard(),
            source: SourceKind::Agent,
            actor_id: None,
        }
    }

    /// Pins the writing scope.
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Anchors the memory on a subject.
    pub fn with_subject(mut self, subject: MemorySubjectBox) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Sets a subtype label.
    pub fn with_subtype(mut self, subtype: impl Into<String>) -> Self {
        self.subtype = Some(subtype.into());
        self
    }

    /// Overrides certainty.
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence;
        self
    }

    /// Overrides retrieval priority.
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance;
        self
    }

    /// Overrides retention.
    pub fn with_retention(mut self, retention: RetentionPolicy) -> Self {
        self.retention = retention;
        self
    }

    /// Declares provenance.
    pub fn from_source(mut self, source: SourceKind, actor_id: impl Into<String>) -> Self {
        self.source = source;
        self.actor_id = Some(actor_id.into());
        self
    }

    /// Validates against configured limits.
    pub fn validated(&self, config: &EngineConfig) -> MemoryResult<()> {
        if self.content.is_empty() {
            return Err(MemoryError::validation("content", "must not be empty"));
        }
        if self.content.char_len() > config.limits.max_content_chars {
            return Err(MemoryError::validation(
                "content",
                format!(
                    "exceeds max_content_chars ({})",
                    config.limits.max_content_chars
                ),
            ));
        }
        Ok(())
    }

    pub(crate) fn into_record(self, default_scope: MemoryScope) -> MemoryRecord {
        let mut record =
            MemoryRecord::new(self.memory_type, self.content)
                .with_scope(self.scope.unwrap_or(default_scope))
                .with_confidence(self.confidence)
                .with_importance(self.importance)
                .with_retention(self.retention);
        record.subtype = self.subtype;
        record.subject = self.subject;
        let mut provenance = Provenance::now(self.source);
        if let Some(actor) = self.actor_id {
            provenance = provenance.with_actor(actor);
        }
        record.provenance = provenance;
        record
    }
}

/// Intent to replace the content of an existing memory.
///
/// The engine never rewrites records in place: it derives a successor
/// linked through `supersedes` and retires the original, preserving
/// full history.
#[derive(Clone, Debug)]
pub struct UpdateRequest {
    /// Record being revised.
    pub id: MemoryId,
    /// Scope of the caller; must contain the record's scope.
    pub scope: MemoryScope,
    /// New payload.
    pub content: MemoryContent,
    /// New certainty, when the caller wants to change it.
    pub confidence: Option<f32>,
    /// New retrieval priority, when the caller wants to change it.
    pub importance: Option<f32>,
}

impl UpdateRequest {
    /// Revises a record's text content.
    pub fn content(id: MemoryId, scope: MemoryScope, content: impl Into<String>) -> Self {
        Self {
            id,
            scope,
            content: MemoryContent::from_text(content),
            confidence: None,
            importance: None,
        }
    }

    /// Overrides the successor's certainty.
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Overrides the successor's retrieval priority.
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = Some(importance);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_request_rejects_empty_content() {
        let req = RememberRequest::new(MemoryType::Semantic, "   ");
        assert!(req.validated(&EngineConfig::default()).is_err());
    }

    #[test]
    fn remember_request_enforces_content_budget() {
        let mut config = EngineConfig::default();
        config.limits.max_content_chars = 8;
        let req = RememberRequest::new(MemoryType::Semantic, "this is far too long");
        let err = req.validated(&config).unwrap_err();
        assert!(matches!(err, MemoryError::Validation { field: "content", .. }));
    }

    #[test]
    fn into_record_applies_defaults_and_overrides() {
        let req = RememberRequest::new(MemoryType::Prospective, "renew cert")
            .with_subtype("commitment")
            .with_confidence(0.8)
            .from_source(SourceKind::User, "u-9");

        let record = req.into_record(MemoryScope::default());
        assert_eq!(record.memory_type, MemoryType::Prospective);
        assert_eq!(record.subtype.as_deref(), Some("commitment"));
        assert_eq!(record.confidence, 0.8);
        assert_eq!(record.provenance.source, SourceKind::User);
        assert_eq!(record.provenance.actor_id.as_deref(), Some("u-9"));
    }

    #[test]
    fn into_record_falls_back_to_default_scope() {
        let default = memory_domain::MemoryScopeBuilder::new()
            .tenant("acme")
            .build();
        let req = RememberRequest::new(MemoryType::Semantic, "fact");
        let record = req.into_record(default.clone());
        assert_eq!(record.scope, default);
    }
}
