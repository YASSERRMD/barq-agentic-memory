//! Extraction: pulling discrete memories from unstructured conversation.

use async_trait::async_trait;
use memory_domain::MemoryResult;
use serde::{Deserialize, Serialize};

/// One memory extracted from a larger text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractedMemory {
    /// The distilled statement ("Customer prefers email").
    pub text: String,
    /// Suggested type for the extracted fact.
    pub memory_type: memory_domain::MemoryType,
    /// Optional refined label.
    pub subtype: Option<String>,
    /// Extractor's confidence in this specific extraction.
    pub confidence: f32,
}

/// Pulls discrete memories out of unstructured text.
///
/// Callers who already know their memories should skip extraction and
/// use `remember()` directly — extraction exists for the "agent had a
/// whole conversation, save what matters" workflow.
#[async_trait]
pub trait ExtractionProvider: Send + Sync {
    /// Human-readable name for logs.
    fn name(&self) -> &str;

    /// Extracts candidate memories from `text`.
    async fn extract(&self, text: &str) -> MemoryResult<Vec<ExtractedMemory>>;
}
