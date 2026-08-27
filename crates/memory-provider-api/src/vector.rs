//! Vector index contract for semantic similarity retrieval.

use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryResult, MemoryScope, MemoryType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A vector attached to a canonical memory record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Canonical memory this embedding belongs to.
    pub memory_id: MemoryId,
    /// Embedding values.
    pub embedding: Vec<f32>,
    /// Embedding model identifier (e.g. "text-embedding-3-small").
    pub model: String,
    /// Model version; mixing versions in one index is a corruption risk,
    /// so every record carries its own stamp.
    pub model_version: String,
    /// Scalar metadata used for pre-filtering at search time.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl VectorRecord {
    /// Stamps a new vector record for a memory.
    pub fn new(
        memory_id: MemoryId,
        embedding: Vec<f32>,
        model: impl Into<String>,
        model_version: impl Into<String>,
    ) -> Self {
        Self {
            memory_id,
            embedding,
            model: model.into(),
            model_version: model_version.into(),
            metadata: HashMap::new(),
        }
    }

    /// Adds a metadata entry for filtered search.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// Metadata equality filters applied before similarity ranking.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataFilter {
    /// Exact-match clauses; all must hold.
    pub equals: HashMap<String, String>,
}

impl MetadataFilter {
    /// True when a record's metadata satisfies every clause.
    pub fn matches(&self, metadata: &HashMap<String, String>) -> bool {
        self.equals.iter().all(|(k, v)| metadata.get(k) == Some(v))
    }
}

/// A top-K similarity query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VectorQuery {
    /// Query embedding; dimension must match the index.
    pub embedding: Vec<f32>,
    /// Number of neighbors to return.
    pub top_k: u32,
    /// Optional scope narrowing mirrored into record metadata.
    pub scope: Option<MemoryScope>,
    /// Optional type restriction.
    pub memory_type: Option<MemoryType>,
    /// Metadata equality pre-filter.
    pub filter: MetadataFilter,
}

impl Default for VectorQuery {
    fn default() -> Self {
        Self {
            embedding: Vec::new(),
            top_k: 10,
            scope: None,
            memory_type: None,
            filter: MetadataFilter::default(),
        }
    }
}

impl VectorQuery {
    /// Validates the structural invariants providers rely on.
    pub fn validated(self) -> MemoryResult<Self> {
        if self.embedding.is_empty() {
            return Err(MemoryError::validation("embedding", "must not be empty"));
        }
        if self.top_k == 0 {
            return Err(MemoryError::validation("top_k", "must be at least one"));
        }
        Ok(self)
    }
}

/// One neighbor returned from similarity search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorMatch {
    /// Canonical memory id of the matched vector.
    pub memory_id: MemoryId,
    /// Similarity score normalized to `[0, 1]` where 1 is identical.
    pub score: f32,
}

/// Replaceable semantic index.
///
/// Implementations must keep the index consistent with the canonical
/// store: upserts and deletes target records by [`MemoryId`], never by
/// provider-native keys.
#[async_trait]
pub trait VectorProvider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Inserts or replaces the vector bound to `record.memory_id`.
    async fn upsert(&self, record: &VectorRecord) -> MemoryResult<()>;

    /// Returns up to `query.top_k` nearest neighbors satisfying filters.
    async fn search(&self, query: &VectorQuery) -> MemoryResult<Vec<VectorMatch>>;

    /// Removes any vector bound to the given memory id. Idempotent.
    async fn delete(&self, memory_id: &MemoryId) -> MemoryResult<()>;

    /// All indexed memory ids, for repairs and consistency checks.
    ///
    /// Similarity search cannot enumerate (zero-score results are
    /// legitimately filtered), so repairs use this instead. Minimal
    /// backends may leave the default empty listing.
    async fn list_ids(&self) -> MemoryResult<Vec<MemoryId>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_filter_requires_every_clause() {
        let mut meta = HashMap::new();
        meta.insert("tenant".to_string(), "acme".to_string());

        let mut f = MetadataFilter::default();
        f.equals.insert("tenant".to_string(), "acme".to_string());
        assert!(f.matches(&meta));

        f.equals.insert("user".to_string(), "u-1".to_string());
        assert!(!f.matches(&meta), "missing clause fails closed");
    }

    #[test]
    fn query_validation_rejects_empty_embedding_and_zero_k() {
        assert!(VectorQuery::default().validated().is_err());
        let q = VectorQuery {
            embedding: vec![0.1, 0.2],
            top_k: 0,
            ..VectorQuery::default()
        };
        assert!(q.validated().is_err());
        let q = VectorQuery {
            embedding: vec![0.1],
            top_k: 3,
            ..VectorQuery::default()
        };
        assert!(q.validated().is_ok());
    }

    #[test]
    fn vector_record_serializes_with_model_stamp() {
        let r = VectorRecord::new(MemoryId::generate(), vec![1.0, 2.0], "m", "v1")
            .with_metadata("type", "semantic");
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"model\":\"m\""));
        let back: VectorRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, r);
    }
}
