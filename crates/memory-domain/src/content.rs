//! Memory payload value types.

use serde::{Deserialize, Serialize};

/// The payload of a memory record.
///
/// `text` is always present because every retrieval path (keyword,
/// vector, graph) ultimately renders language. `structured` optionally
/// carries caller-supplied structure so the engine can work with zero
/// LLM dependency.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryContent {
    /// Natural-language text of the memory.
    pub text: String,
    /// Optional structured representation (JSON) supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<serde_json::Value>,
    /// Free-form labels used by filtering; not part of the text itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl MemoryContent {
    /// Text-only content.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured: None,
            tags: Vec::new(),
        }
    }

    /// Text plus structured payload.
    pub fn with_structured(mut self, structured: serde_json::Value) -> Self {
        self.structured = Some(structured);
        self
    }

    /// Adds tags.
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Length of the text in characters, for budgeting prompts.
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// True when there is nothing meaningful to remember.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.structured.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_only_content_has_no_optional_fields_serialized() {
        let c = MemoryContent::from_text("Customer prefers email.");
        let json = serde_json::to_string(&c).expect("serialize");
        assert_eq!(json, r#"{"text":"Customer prefers email."}"#);
    }

    #[test]
    fn structured_payload_survives_roundtrip() {
        let c = MemoryContent::from_text("db choice").with_structured(json!({
            "project": "atlas",
            "database": "postgres"
        }));
        let back: MemoryContent =
            serde_json::from_str(&serde_json::to_string(&c).expect("serialize"))
                .expect("deserialize");
        assert_eq!(back, c);
    }

    #[test]
    fn emptiness_requires_trimmed_text_or_structure() {
        assert!(MemoryContent::from_text("   \n").is_empty());
        assert!(!MemoryContent::from_text("hello").is_empty());
        assert!(
            !MemoryContent::default()
                .with_structured(json!(null))
                .is_empty()
        );
    }

    #[test]
    fn char_len_counts_characters_not_bytes() {
        let c = MemoryContent::from_text("héllo");
        assert_eq!(c.char_len(), 5);
    }
}
