//! Embedding abstraction: turn text into vectors for semantic recall.
//!
//! Implementations stamp every vector with a model name and version so
//! indexes can refuse to mix model generations. The hashing embedder
//! keeps the engine fully LLM-free for embedded deployments and tests.

use async_trait::async_trait;
use memory_domain::MemoryResult;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Produces embeddings for text payloads.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Model identifier stamped onto vectors.
    fn model(&self) -> &str;

    /// Model version; mixing versions in one index is rejected.
    fn model_version(&self) -> &str;

    /// Vector dimensionality.
    fn dimensions(&self) -> usize;

    /// Embeds a batch of texts, preserving order.
    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>>;
}

/// Deterministic feature-hashing embedder.
///
/// Tokenizes on non-alphanumeric boundaries, folds each token into the
/// vector via signed hashing, then L2-normalizes. No network, no
/// model download: semantic recall works out of the box in embedded
/// mode, and real embedding models can replace it without touching the
/// engine (same trait, same stamps).
pub struct HashingEmbedder {
    dimensions: usize,
}

impl HashingEmbedder {
    /// Creates an embedder with the requested dimensionality.
    ///
    /// Dimensions are fixed per index; callers should pick once
    /// (e.g. 384) and stay consistent for the lifetime of an index.
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

#[async_trait]
impl EmbeddingProvider for HashingEmbedder {
    fn model(&self) -> &str {
        "barq-hashing"
    }

    fn model_version(&self) -> &str {
        "1"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, texts: &[String]) -> MemoryResult<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                // Feature hashing with two independent votes per token:
                // single-bucket collisions made unrelated texts score
                // identically at small dimensions, so each token now
                // spreads across two buckets with independent signs.
                let mut vec = vec![0.0_f32; self.dimensions];
                for token in tokenize(text) {
                    for salt in [0u8, 1] {
                        let mut hasher = DefaultHasher::new();
                        token.hash(&mut hasher);
                        salt.hash(&mut hasher);
                        let raw = hasher.finish();
                        let bucket = if self.dimensions == 0 {
                            0
                        } else {
                            (raw % self.dimensions as u64) as usize
                        };
                        vec[bucket] += if raw >> 63 == 1 { -0.5 } else { 0.5 };
                    }
                }
                normalize(&mut vec);
                vec
            })
            .collect())
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && t.len() > 1)
        .map(str::to_string)
        .collect()
}

fn normalize(vec: &mut [f32]) {
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
}

/// Cosine similarity normalized to `[0, 1]` where 1 means identical
/// direction; used by in-memory search and tests.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = normalize_magnitude(a);
    let nb = normalize_magnitude(b);
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    ((dot / (na * nb)) / 2.0 + 0.5).clamp(0.0, 1.0)
}

fn normalize_magnitude(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn identical_texts_have_identical_vectors() {
        let e = HashingEmbedder::new(64);
        let v = e
            .embed(&["customer prefers email".to_string()])
            .await
            .expect("embed");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), 64);

        let w = e
            .embed(&["customer prefers email".to_string()])
            .await
            .expect("embed");
        assert_eq!(cosine_similarity(&v[0], &w[0]), 1.0);
    }

    #[tokio::test]
    async fn related_text_scores_higher_than_unrelated() {
        let e = HashingEmbedder::new(256);
        let base = e
            .embed(&["project atlas uses postgresql".to_string()])
            .await
            .expect("embed")
            .remove(0);
        let similar = e
            .embed(&["atlas uses postgresql daily".to_string()])
            .await
            .expect("embed")
            .remove(0);
        let unrelated = e
            .embed(&["recipe for sourdough bread".to_string()])
            .await
            .expect("embed")
            .remove(0);

        let s_similar = cosine_similarity(&base, &similar);
        let s_unrelated = cosine_similarity(&base, &unrelated);
        assert!(
            s_similar > s_unrelated,
            "keyword overlap must dominate: {s_similar} vs {s_unrelated}"
        );
    }

    #[test]
    fn stamps_are_stable_and_dimensions_reported() {
        let e = HashingEmbedder::new(384);
        assert_eq!(e.model(), "barq-hashing");
        assert_eq!(e.model_version(), "1");
        assert_eq!(e.dimensions(), 384);
    }

    #[test]
    fn cosine_handles_zero_vectors_safely() {
        assert_eq!(cosine_similarity(&[0.0; 4], &[1.0, 0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn tokenizer_drops_punctuation_and_single_chars() {
        let tokens = tokenize("Hello, world! I'm barq-agent v7");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(!tokens.contains(&"i".to_string()));
    }
}
