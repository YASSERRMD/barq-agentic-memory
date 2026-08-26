//! Node.js bindings core: JSON-in/JSON-out functions over engine handles.
//!
//! Class-method registration is broken by a napi_derive/rustc name-
//! extraction issue on this toolchain (methods expand to empty
//! property names), so the native layer exposes plain functions plus
//! an opaque handle registry; `index.js` wraps them into the familiar
//! `Memory` class with identical semantics.

use memory_core::{MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::{
    config::{EmbeddingConfig, EngineConfig, StoreConfig, VectorStoreConfig},
    MemoryId, MemoryQuery, MemoryScope, MemoryType,
};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}

fn engines() -> &'static Mutex<HashMap<u64, Arc<MemoryEngine>>> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<MemoryEngine>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_handle() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn parse_memory_type(name: Option<String>) -> Result<MemoryType> {
    match name.as_deref().unwrap_or("semantic") {
        "working" => Ok(MemoryType::Working),
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        "prospective" => Ok(MemoryType::Prospective),
        other => Err(Error::new(
            Status::InvalidArg,
            format!("unknown memory_type '{other}'"),
        )),
    }
}

fn scope_from_json(json: &str) -> Result<MemoryScope> {
    if json.trim().is_empty() {
        return Ok(MemoryScope::default());
    }
    serde_json::from_str(json)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid scope: {e}")))
}

/// Opens an embedded engine; returns its handle id.
#[napi]
pub fn memory_open(path: Option<String>, namespace: Option<String>) -> Result<f64> {
    let mut config = EngineConfig {
        namespace: namespace.unwrap_or_else(|| "default".into()),
        // Semantic recall out of the box: built-in hashing embedder.
        vector: Some(VectorStoreConfig::InMemory),
        embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
        ..EngineConfig::default()
    };
    config.store = match &path {
        Some(p) if !p.trim().is_empty() => StoreConfig::Local { path: PathBuf::from(p) },
        _ => StoreConfig::Memory,
    };
    let engine = block_on(MemoryEngine::from_config(config))
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    let handle = next_handle() as f64;
    engines()
        .lock()
        .expect("poisoned")
        .insert(handle as u64, Arc::new(engine));
    Ok(handle)
}

/// Drops an engine handle.
#[napi]
pub fn memory_close(handle: f64) {
    engines().lock().expect("poisoned").remove(&(handle as u64));
}

/// Remembers text; request is JSON: {text, type?, tenantId?, userId?,
/// agentId?, sessionId?, confidence?}. Returns the record as JSON.
#[napi]
pub fn memory_remember(handle: f64, request_json: String) -> Result<String> {
    #[derive(serde::Deserialize)]
    #[serde(default)]
    struct Req {
        text: String,
        r#type: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
        agent_id: Option<String>,
        session_id: Option<String>,
        confidence: f32,
    }
    impl Default for Req {
        fn default() -> Self {
            Self {
                text: String::new(),
                r#type: "semantic".into(),
                tenant_id: None,
                user_id: None,
                agent_id: None,
                session_id: None,
                confidence: 0.5,
            }
        }
    }
    let req: Req = serde_json::from_str(&request_json)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid request: {e}")))?;
    if req.text.trim().is_empty() {
        return Err(Error::new(Status::InvalidArg, "text must not be empty"));
    }

    let kind = parse_memory_type(Some(req.r#type.clone()))?;
    let mut scope = MemoryScope::default();
    if req.tenant_id.is_some()
        || req.user_id.is_some()
        || req.agent_id.is_some()
        || req.session_id.is_some()
    {
        let mut b = MemoryScope::builder();
        if let Some(t) = &req.tenant_id {
            b = b.tenant(t);
        }
        if let Some(u) = &req.user_id {
            b = b.user(u);
        }
        if let Some(a) = &req.agent_id {
            b = b.agent(a);
        }
        if let Some(s) = &req.session_id {
            b = b.session(s);
        }
        scope = b.build();
    }

    let request = RememberRequest::new(kind, req.text.clone())
        .with_scope(scope)
        .with_confidence(req.confidence);
    let engine = engines()
        .lock()
        .expect("poisoned")
        .get(&(handle as u64))
        .cloned()
        .ok_or_else(|| Error::new(Status::InvalidArg, "unknown engine handle"))?;
    let saved = block_on(engine.remember(request))
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    serde_json::to_string(&saved).map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}

/// Search: {query, tenantId?, userId?, limit?}. Returns records JSON.
#[napi]
pub fn memory_search(handle: f64, query_json: String) -> Result<String> {
    #[derive(serde::Deserialize)]
    #[serde(default)]
    struct Req {
        query: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
        limit: u32,
    }
    impl Default for Req {
        fn default() -> Self {
            Self { query: String::new(), tenant_id: None, user_id: None, limit: 10 }
        }
    }
    let req: Req = serde_json::from_str(&query_json)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid query: {e}")))?;
    if req.query.trim().is_empty() {
        return Err(Error::new(Status::InvalidArg, "query must not be empty"));
    }

    let mut q = MemoryQuery::default()
        .with_text(req.query.clone())
        .with_limit(req.limit.max(1));
    if req.tenant_id.is_some() || req.user_id.is_some() {
        let mut b = MemoryScope::builder();
        if let Some(t) = &req.tenant_id {
            b = b.tenant(t);
        }
        if let Some(u) = &req.user_id {
            b = b.user(u);
        }
        q = q.with_scope(b.build());
    }

    let engine = engines()
        .lock()
        .expect("poisoned")
        .get(&(handle as u64))
        .cloned()
        .ok_or_else(|| Error::new(Status::InvalidArg, "unknown engine handle"))?;
    let hits = block_on(engine.search(q))
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    serde_json::to_string(&hits).map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}

/// Hybrid recall; degrades to search without a vector backend.
#[napi]
pub fn memory_recall(handle: f64, query_json: String) -> Result<String> {
    #[derive(serde::Deserialize)]
    #[serde(default)]
    struct Req {
        query: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
        limit: u32,
    }
    impl Default for Req {
        fn default() -> Self {
            Self { query: String::new(), tenant_id: None, user_id: None, limit: 10 }
        }
    }
    let req: Req = serde_json::from_str(&query_json)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid query: {e}")))?;

    let mut request = memory_retrieval::RecallRequest::new(req.query.clone())
        .with_budget(req.limit.max(1));
    if req.tenant_id.is_some() || req.user_id.is_some() {
        let mut b = MemoryScope::builder();
        if let Some(t) = &req.tenant_id {
            b = b.tenant(t);
        }
        if let Some(u) = &req.user_id {
            b = b.user(u);
        }
        request = request.with_scope(b.build());
    }

    let engine = engines()
        .lock()
        .expect("poisoned")
        .get(&(handle as u64))
        .cloned()
        .ok_or_else(|| Error::new(Status::InvalidArg, "unknown engine handle"))?;
    match block_on(engine.recall(&request)) {
        Ok(ranked) => {
            let records: Vec<&memory_domain::MemoryRecord> =
                ranked.iter().map(|c| &c.record).collect();
            serde_json::to_string(&records)
                .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
        }
        Err(memory_domain::MemoryError::Unsupported(_)) => {
            let search_json = serde_json::json!({
                "query": req.query,
                "tenant_id": req.tenant_id,
                "user_id": req.user_id,
                "limit": req.limit,
            });
            memory_search(handle, search_json.to_string())
        }
        Err(e) => Err(Error::new(Status::GenericFailure, e.to_string())),
    }
}

/// Update content by supersession: {id, text}. Returns successor JSON.
#[napi]
pub fn memory_update(handle: f64, update_json: String) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Req {
        id: String,
        text: String,
    }
    let req: Req = serde_json::from_str(&update_json)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid update: {e}")))?;
    let mid = MemoryId::parse(&req.id)
        .map_err(|_| Error::new(Status::InvalidArg, format!("invalid id '{}'", req.id)))?;
    let request = UpdateRequest::content(mid, Default::default(), req.text);

    let engine = engines()
        .lock()
        .expect("poisoned")
        .get(&(handle as u64))
        .cloned()
        .ok_or_else(|| Error::new(Status::InvalidArg, "unknown engine handle"))?;
    let successor = block_on(engine.update(request))
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    serde_json::to_string(&successor).map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}

/// Soft-delete: {id}. Returns whether anything changed.
#[napi]
pub fn memory_forget(handle: f64, id: String) -> Result<bool> {
    let mid = MemoryId::parse(&id)
        .map_err(|_| Error::new(Status::InvalidArg, format!("invalid id '{id}'")))?;
    let engine = engines()
        .lock()
        .expect("poisoned")
        .get(&(handle as u64))
        .cloned()
        .ok_or_else(|| Error::new(Status::InvalidArg, "unknown engine handle"))?;
    block_on(engine.forget(mid, &Default::default()))
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}

/// Supersession chain, oldest first: {id}.
#[napi]
pub fn memory_history(handle: f64, id: String) -> Result<String> {
    let mid = MemoryId::parse(&id)
        .map_err(|_| Error::new(Status::InvalidArg, format!("invalid id '{id}'")))?;
    let engine = engines()
        .lock()
        .expect("poisoned")
        .get(&(handle as u64))
        .cloned()
        .ok_or_else(|| Error::new(Status::InvalidArg, "unknown engine handle"))?;
    let chain = block_on(engine.history(mid, &Default::default()))
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    serde_json::to_string(&chain).map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}
