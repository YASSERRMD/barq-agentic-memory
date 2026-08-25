//! The entity a memory is about.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Names the subject of a memory, e.g. "Project Atlas" or a user.
///
/// Subjects give structured lookups an anchor: the retrieval planner can
/// try an exact subject match before falling back to vector search.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemorySubject {
    /// Coarse category of the subject ("project", "person", "tool", ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// Stable identifier of the subject within its category.
    pub entity_id: String,
    /// Human-friendly label used for display and keyword search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl MemorySubject {
    /// Creates a subject with only its identifier.
    pub fn new(entity_id: impl Into<String>) -> Self {
        Self {
            entity_type: None,
            entity_id: entity_id.into(),
            display_name: None,
        }
    }

    /// Attaches a category to the subject.
    pub fn with_type(mut self, entity_type: impl Into<String>) -> Self {
        self.entity_type = Some(entity_type.into());
        self
    }

    /// Attaches a display name to the subject.
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    /// Canonical lookup key: `type:id` when typed, bare `id` otherwise.
    ///
    /// Normalizing the key here keeps dedup and exact-lookup consistent
    /// across providers instead of every backend inventing its own form.
    pub fn canonical_key(&self) -> String {
        match &self.entity_type {
            Some(t) => format!("{}:{}", t.trim().to_lowercase(), self.entity_id.trim()),
            None => self.entity_id.trim().to_string(),
        }
    }
}

impl fmt::Display for MemorySubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = &self.display_name {
            write!(f, "{} ({})", name, self.canonical_key())
        } else {
            f.write_str(&self.canonical_key())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_normalizes_case_and_whitespace() {
        let s = MemorySubject::new(" Atlas ")
            .with_type(" Project ")
            .with_display_name("Project Atlas");
        assert_eq!(s.canonical_key(), "project:Atlas");
        assert_eq!(s.canonical_key(), s.clone().canonical_key());
    }

    #[test]
    fn untyped_subjects_use_bare_ids() {
        let s = MemorySubject::new("user-42");
        assert_eq!(s.canonical_key(), "user-42");
    }

    #[test]
    fn display_prefers_human_name() {
        let s = MemorySubject::new("atlas").with_display_name("Project Atlas");
        assert_eq!(s.to_string(), "Project Atlas (atlas)");
    }

    #[test]
    fn serde_roundtrip() {
        let s = MemorySubject::new("postgres").with_type("database");
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(!json.contains("display_name"));
        let back: MemorySubject = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, s);
    }
}
