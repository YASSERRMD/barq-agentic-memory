//! The classification contract and its value types.

use async_trait::async_trait;
use memory_domain::{MemoryResult, MemoryType};
use serde::{Deserialize, Serialize};

/// Input to a classifier: what was observed and where it belongs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassifierInput {
    /// The text to classify.
    pub text: String,
    /// Optional caller hint; when present the classifier should not
    /// override it without cause.
    pub hinted_type: Option<MemoryType>,
    /// Free-form context (session summary, preceding turns).
    pub context: Option<String>,
}

impl ClassifierInput {
    /// Classify bare text.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            hinted_type: None,
            context: None,
        }
    }
}

/// A classification decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    /// Chosen memory type.
    pub memory_type: MemoryType,
    /// Refined label ("preference", "commitment", ...), if confident.
    pub subtype: Option<String>,
    /// Certainty of the decision in [0, 1].
    pub confidence: f32,
    /// Keywords worth indexing.
    pub keywords: Vec<String>,
}

impl Classification {
    /// Decision that defers entirely to caller-supplied structure.
    ///
    /// This is how "caller-supplied structured memory" stays a first
    /// class path with zero LLM involvement.
    pub fn passthrough(memory_type: MemoryType) -> Self {
        Self {
            memory_type,
            subtype: None,
            confidence: 1.0,
            keywords: Vec::new(),
        }
    }
}

/// Decides what kind of memory an observation is.
///
/// Implementations range from keyword rules to local models to remote
/// LLMs; the engine only depends on this trait.
#[async_trait]
pub trait MemoryClassifier: Send + Sync {
    /// Human-readable name for logs.
    fn name(&self) -> &str;

    /// Classifies one input.
    async fn classify(&self, input: &ClassifierInput) -> MemoryResult<Classification>;
}
