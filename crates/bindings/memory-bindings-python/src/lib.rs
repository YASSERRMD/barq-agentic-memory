//! Python bindings for the Barq memory engine.
//!
//! ```python
//! from agent_memory import Memory
//! memory = Memory("./data")
//! memory.remember("Customer prefers email.", user_id="123")
//! memory.recall("How should I contact this customer?", user_id="123")
//! ```
//!
//! Embedded mode only: the binding owns a local engine. Server mode
//! clients arrive with the SDK phase.

use memory_core::{MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::{
    config::{EngineConfig, StoreConfig},
    MemoryId, MemoryQuery, MemoryScope, MemoryScopeBuilder, MemoryType,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;

/// A single memory result handed back to Python as a plain dict.
fn record_to_dict(py: Python<'_>, r: &memory_domain::MemoryRecord) -> PyResult<PyObject> {
    let dict = pyo3::types::PyDict::new(py);
    dict.set_item("id", r.id.to_string())?;
    dict.set_item("type", r.memory_type.as_str())?;
    dict.set_item("text", r.content.text.clone())?;
    if let Some(subject) = &r.subject {
        dict.set_item("subject", subject.canonical_key())?;
    }
    dict.set_item("status", r.status.to_string())?;
    dict.set_item("version", r.version)?;
    dict.set_item(
        "created_at",
        r.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )?;
    Ok(dict.into())
}

/// Block on an async engine call from sync Python.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    // A per-call runtime keeps the binding simple; the GIL serializes
    // access anyway at this layer.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}

/// The embedded memory engine.
#[pyclass]
struct Memory {
    engine: Arc<MemoryEngine>,
}

#[pymethods]
impl Memory {
    /// Opens an embedded engine backed by `path` (or in-memory when
    /// omitted).
    #[new]
    #[pyo3(signature = (path=None, namespace="default"))]
    fn new(path: Option<String>, namespace: &str) -> PyResult<Self> {
        let mut config = EngineConfig {
            namespace: namespace.to_string(),
            // Semantic recall works out of the box: feature-hashing
            // embedder needs no model download and no network.
            vector: Some(memory_domain::config::VectorStoreConfig::InMemory),
            embedding: Some(memory_domain::config::EmbeddingConfig::Hashing {
                dimensions: 256,
            }),
            ..EngineConfig::default()
        };
        config.store = match path {
            Some(p) => StoreConfig::Local { path: PathBuf::from(p) },
            None => StoreConfig::Memory,
        };
        let engine = block_on(MemoryEngine::from_config(config))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Remembers text; optional scoping by tenant/user/agent/session.
    #[pyo3(signature = (text, tenant_id=None, user_id=None, agent_id=None,
                        session_id=None, memory_type="semantic",
                        confidence=0.5))]
    fn remember(
        &self,
        py: Python<'_>,
        text: &str,
        tenant_id: Option<&str>,
        user_id: Option<&str>,
        agent_id: Option<&str>,
        session_id: Option<&str>,
        memory_type: &str,
        confidence: f32,
    ) -> PyResult<PyObject> {
        if text.trim().is_empty() {
            return Err(PyValueError::new_err("text must not be empty"));
        }
        let kind = parse_memory_type(memory_type)?;
        let mut scope = MemoryScopeBuilder::new();
        if let Some(t) = tenant_id { scope = scope.tenant(t); }
        if let Some(u) = user_id { scope = scope.user(u); }
        if let Some(a) = agent_id { scope = scope.agent(a); }
        if let Some(s) = session_id { scope = scope.session(s); }

        let request =
            RememberRequest::new(kind, text).with_scope(scope.build()).with_confidence(confidence);
        let engine = self.engine.clone();
        let saved = py
            .allow_threads(move || block_on(async move { engine.remember(request).await }))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        record_to_dict(py, &saved)
    }

    /// Keyword + filtered search over canonical records.
    #[pyo3(signature = (query, tenant_id=None, user_id=None, limit=10))]
    fn search(
        &self,
        py: Python<'_>,
        query: &str,
        tenant_id: Option<&str>,
        user_id: Option<&str>,
        limit: u32,
    ) -> PyResult<Vec<PyObject>> {
        let mut q = MemoryQuery::default().with_text(query).with_limit(limit.max(1));
        if tenant_id.is_some() || user_id.is_some() {
            let mut b = MemoryScopeBuilder::new();
            if let Some(t) = tenant_id { b = b.tenant(t); }
            if let Some(u) = user_id { b = b.user(u); }
            q = q.with_scope(b.build());
        }
        let engine = self.engine.clone();
        let hits = py
            .allow_threads(move || block_on(async move { engine.search(q).await }))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        hits.iter().map(|r| record_to_dict(py, r)).collect()
    }

    /// Hybrid recall: semantic similarity when embeddings are enabled,
    /// otherwise falls back to keyword search.
    #[pyo3(signature = (query, tenant_id=None, user_id=None, limit=10))]
    fn recall(
        &self,
        py: Python<'_>,
        query: &str,
        tenant_id: Option<&str>,
        user_id: Option<&str>,
        limit: u32,
    ) -> PyResult<Vec<PyObject>> {
        if query.trim().is_empty() {
            return Err(PyValueError::new_err("query must not be empty"));
        }
        let mut scope = MemoryScope::default();
        if tenant_id.is_some() || user_id.is_some() {
            let mut b = MemoryScopeBuilder::new();
            if let Some(t) = tenant_id { b = b.tenant(t); }
            if let Some(u) = user_id { b = b.user(u); }
            scope = b.build();
        }

        let engine = self.engine.clone();
        let request = memory_retrieval::RecallRequest::new(query)
            .with_scope(scope)
            .with_budget(limit.max(1));
        let ranked = py
            .allow_threads(move || block_on(async move { engine.recall(&request).await }));

        match ranked {
            Ok(hits) => hits.iter().map(|c| record_to_dict(py, &c.record)).collect(),
            Err(memory_domain::MemoryError::Unsupported(_)) => {
                // No vector backend configured: degrade to keyword search.
                self.search(py, query, tenant_id, user_id, limit)
            }
            Err(e) => Err(PyValueError::new_err(e.to_string())),
        }
    }

    /// Updates content by supersession; history is preserved.
    fn update(&self, py: Python<'_>, id: &str, new_text: &str) -> PyResult<PyObject> {
        let mid = MemoryId::parse(id)
            .map_err(|_| PyValueError::new_err(format!("invalid memory id '{id}'")))?;
        let request = UpdateRequest::content(mid, MemoryScope::default(), new_text);
        let engine = self.engine.clone();
        let successor = py
            .allow_threads(move || block_on(async move { engine.update(request).await }))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        record_to_dict(py, &successor)
    }

    /// Soft-deletes a memory.
    fn forget(&self, py: Python<'_>, id: &str) -> PyResult<bool> {
        let mid = MemoryId::parse(id)
            .map_err(|_| PyValueError::new_err(format!("invalid memory id '{id}'")))?;
        let engine = self.engine.clone();
        py.allow_threads(move || {
            block_on(async move { engine.forget(mid, &Default::default()).await })
        })
        .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// The supersession chain for one memory, oldest first.
    fn history(&self, py: Python<'_>, id: &str) -> PyResult<Vec<PyObject>> {
        let mid = MemoryId::parse(id)
            .map_err(|_| PyValueError::new_err(format!("invalid memory id '{id}'")))?;
        let engine = self.engine.clone();
        let chain = py
            .allow_threads(move || block_on(async move { engine.history(mid, &Default::default()).await }))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        chain.iter().map(|r| record_to_dict(py, r)).collect()
    }
}

fn parse_memory_type(name: &str) -> PyResult<MemoryType> {
    match name {
        "working" => Ok(MemoryType::Working),
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        "prospective" => Ok(MemoryType::Prospective),
        other => Err(PyValueError::new_err(format!(
            "unknown memory_type '{other}' (use working/episodic/semantic/procedural/prospective)"
        ))),
    }
}

/// Module entrypoint.
#[pymodule]
fn agent_memory(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Memory>()?;
    Ok(())
}

// Rust-side tests cover wrapper logic that does not require a Python
// interpreter round-trip; full wheel smoke tests run under maturin via
// scripts/test_python_binding.sh (opt-in like PG/Redis).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_parsing_covers_all_five() {
        for name in ["working", "episodic", "semantic", "procedural", "prospective"] {
            assert!(parse_memory_type(name).is_ok());
        }
        assert!(parse_memory_type("quantum").is_err());
    }
}
