//! Deterministic embedder plus scripted classifier/extractor mocks.

use async_trait::async_trait;
use memory_classifier::{
    Classification, ClassifierInput, ExtractedMemory, ExtractionProvider, MemoryClassifier,
};
use memory_domain::MemoryResult;
use memory_provider_api::{EmbeddingProvider, HashingEmbedder};
use std::collections::HashMap;
use std::sync::Mutex;

/// Embedder that delegates to the hashing embedder (deterministic) and
/// can override vectors for specific texts so suites control similarity
/// exactly.
pub struct MockEmbedder {
    inner: HashingEmbedder,
    overrides: Mutex<HashMap<String, Vec<f32>>>,
    pub calls: Mutex<Vec<String>>,
}

impl MockEmbedder {
    pub fn new(dimensions: usize) -> Self {
        Self {
            inner: HashingEmbedder::new(dimensions),
            overrides: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Forces the embedding used for one exact text.
    pub fn override_text(&self, text: &str, vector: Vec<f32>) {
        self.overrides
            .lock()
            .expect("poisoned")
            .insert(text.to_string(), vector);
    }

    /// Makes two texts embed identically (similarity 1.0).
    pub fn make_identical(&self, a: &str, b: &str) {
        let vec = self.inner_blocking(a);
        self.override_text(b, vec);
    }

    fn inner_blocking(&self, text: &str) -> Vec<f32> {
        // A dedicated thread: callers may already be inside a runtime,
        // and nested block_on panics. The hash embedder is pure sync
        // work, so this is deterministic and instant.
        let dims = self.inner.dimensions();
        let text = text.to_string();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(HashingEmbedder::new(dims).embed(std::slice::from_ref(&text)))
                .expect("hash embed")
                .remove(0)
        })
        .join()
        .expect("thread")
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    fn model(&self) -> &str {
        "mock"
    }
    fn model_version(&self) -> &str {
        "1"
    }
    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>> {
        let mut logged = self.calls.lock().expect("poisoned").clone();
        logged.extend(texts.iter().cloned());
        *self.calls.lock().expect("poisoned") = logged;

        let defaults = self.inner.embed(texts).await?;
        let overrides = self.overrides.lock().expect("poisoned");
        Ok(texts
            .iter()
            .zip(defaults)
            .map(|(text, default)| overrides.get(text).cloned().unwrap_or(default))
            .collect())
    }
}

/// Scripted classifier: pops preset responses; falls back to a fixed
/// decision when the script empties.
pub struct MockClassifier {
    pub script: Mutex<std::collections::VecDeque<Classification>>,
    pub fallback: Classification,
    pub inputs: Mutex<Vec<String>>,
}

impl MockClassifier {
    pub fn scripted(fallback: Classification, responses: Vec<Classification>) -> Self {
        Self {
            script: Mutex::new(responses.into()),
            fallback,
            inputs: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl MemoryClassifier for MockClassifier {
    fn name(&self) -> &str {
        "mock-classifier"
    }

    async fn classify(&self, input: &ClassifierInput) -> MemoryResult<Classification> {
        self.inputs
            .lock()
            .expect("poisoned")
            .push(input.text.clone());
        let next = self.script.lock().expect("poisoned").pop_front();
        Ok(next.unwrap_or_else(|| self.fallback.clone()))
    }
}

/// Scripted extractor returning a fixed set of memories.
pub struct MockExtractor {
    pub canned: Vec<ExtractedMemory>,
}

#[async_trait]
impl ExtractionProvider for MockExtractor {
    fn name(&self) -> &str {
        "mock-extractor"
    }

    async fn extract(&self, _text: &str) -> MemoryResult<Vec<ExtractedMemory>> {
        Ok(self.canned.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::MemoryType;

    #[tokio::test]
    async fn overrides_force_similarity_exactly() {
        let e = MockEmbedder::new(64);
        e.make_identical("statement one", "statement two");
        let v = e
            .embed(&["statement one".into(), "statement two".into()])
            .await
            .unwrap();
        assert_eq!(
            memory_provider_api::cosine_similarity(&v[0], &v[1]),
            1.0,
            "overridden texts embed identically"
        );
    }

    #[tokio::test]
    async fn classifier_script_pops_then_falls_back() {
        use memory_classifier::MemoryClassifier as _;
        let c = MockClassifier::scripted(
            Classification::passthrough(MemoryType::Semantic),
            vec![Classification {
                memory_type: MemoryType::Prospective,
                subtype: Some("commitment".into()),
                confidence: 0.9,
                keywords: vec![],
            }],
        );
        let first = c
            .classify(&ClassifierInput::text("anything"))
            .await
            .unwrap();
        assert_eq!(first.memory_type, MemoryType::Prospective);
        let second = c
            .classify(&ClassifierInput::text("anything"))
            .await
            .unwrap();
        assert_eq!(second.confidence, 1.0, "fallback after script empties");
    }
}
