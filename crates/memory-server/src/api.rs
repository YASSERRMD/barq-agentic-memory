//! Route handlers and wiring.

use crate::dto::{
    ApiError, CreateMemoryBody, MemoryDto, ProvenanceDto, QueryBody, ScoredMemoryDto,
    UpdateMemoryBody,
};
use crate::state::ServerState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::Arc;

use memory_core::{MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::{MemoryId, MemoryQuery, MemoryScopeBuilder, MemoryType};

/// Builds the full application router.
pub fn router(state: ServerState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/memories", post(create_memory))
        .route(
            "/v1/memories/{id}",
            get(get_memory).patch(patch_memory).delete(delete_memory),
        )
        .route("/v1/memories/{id}/history", get(get_history))
        .route("/v1/memories/{id}/provenance", get(get_provenance))
        .route("/v1/recall", post(recall))
        .route("/v1/search", post(search))
        .with_state(state)
}

/// Serves until shutdown; used by the binary and tests.
pub async fn serve(engine: MemoryEngine, addr: std::net::SocketAddr) -> std::io::Result<()> {
    let app = router(ServerState::new(engine));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

async fn health() -> StatusCode {
    StatusCode::OK
}

fn map_error(e: memory_domain::MemoryError) -> (StatusCode, Json<ApiError>) {
    use memory_domain::MemoryError;
    match e {
        MemoryError::NotFound { memory_id } => ApiError::not_found(&memory_id.to_string()),
        MemoryError::Validation { reason, .. } => ApiError::validation(reason),
        MemoryError::Unsupported(m) => ApiError::validation(m),
        MemoryError::ProviderMissing { capability } => {
            ApiError::internal(format!("no provider for capability '{capability}'"))
        }
        other => ApiError::internal(other.to_string()),
    }
}

fn parse_type(name: Option<&str>) -> Result<MemoryType, (StatusCode, Json<ApiError>)> {
    match name.unwrap_or("semantic") {
        "working" => Ok(MemoryType::Working),
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        "prospective" => Ok(MemoryType::Prospective),
        other => Err(ApiError::validation(format!(
            "unknown memory_type '{other}'"
        ))),
    }
}

fn scope_from(
    body_tenant: Option<&str>,
    body_user: Option<&str>,
    params: &HashMap<String, String>,
) -> memory_domain::MemoryScope {
    let tenant = body_tenant
        .map(str::to_string)
        .or_else(|| params.get("tenant_id").cloned());
    let user = body_user
        .map(str::to_string)
        .or_else(|| params.get("user_id").cloned());
    let mut b = MemoryScopeBuilder::new();
    if let Some(t) = tenant {
        b = b.tenant(t);
    }
    if let Some(u) = user {
        b = b.user(u);
    }
    b.build()
}

async fn create_memory(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<CreateMemoryBody>,
) -> Result<(StatusCode, Json<MemoryDto>), (StatusCode, Json<ApiError>)> {
    if body.text.trim().is_empty() {
        return Err(ApiError::validation("text must not be empty"));
    }
    let kind = parse_type(body.memory_type.as_deref())?;
    let mut scope = MemoryScopeBuilder::new();
    if let Some(t) = &body.tenant_id {
        scope = scope.tenant(t.clone());
    }
    if let Some(u) = &body.user_id {
        scope = scope.user(u.clone());
    }
    if let Some(a) = &body.agent_id {
        scope = scope.agent(a.clone());
    }
    if let Some(s) = &body.session_id {
        scope = scope.session(s.clone());
    }

    let mut request = RememberRequest::new(kind, body.text.clone()).with_scope(scope.build());
    if let Some(c) = body.confidence {
        request = request.with_confidence(c);
    }

    let saved = state.engine.remember(request).await.map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(MemoryDto::from_record(&saved))))
}

async fn get_memory(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<MemoryDto>, (StatusCode, Json<ApiError>)> {
    let mid =
        MemoryId::parse(&id).map_err(|_| ApiError::validation(format!("invalid id '{id}'")))?;
    let scope = scope_from(None, None, &params);
    let record = state
        .engine
        .recall_exact(mid, &scope)
        .await
        .map_err(map_error)?
        // Tombstoned memories read as absent: forgetting is hiding,
        // history stays addressable through /history.
        .filter(|r| r.status != memory_domain::MemoryStatus::Deleted)
        .ok_or_else(|| ApiError::not_found(&id))?;
    Ok(Json(MemoryDto::from_record(&record)))
}

async fn patch_memory(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<UpdateMemoryBody>,
) -> Result<Json<MemoryDto>, (StatusCode, Json<ApiError>)> {
    let mid =
        MemoryId::parse(&id).map_err(|_| ApiError::validation(format!("invalid id '{id}'")))?;
    if body.text.trim().is_empty() {
        return Err(ApiError::validation("text must not be empty"));
    }
    let scope = scope_from(None, None, &params);
    let mut request = UpdateRequest::content(mid, scope, body.text.clone());
    if let Some(c) = body.confidence {
        request = request.with_confidence(c);
    }
    if let Some(i) = body.importance {
        request = request.with_importance(i);
    }
    let successor = state.engine.update(request).await.map_err(map_error)?;
    Ok(Json(MemoryDto::from_record(&successor)))
}

async fn delete_memory(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mid =
        MemoryId::parse(&id).map_err(|_| ApiError::validation(format!("invalid id '{id}'")))?;
    let scope = scope_from(None, None, &params);
    if params.get("hard").map(|v| v == "true").unwrap_or(false) {
        state.engine.purge(mid, &scope).await.map_err(map_error)?;
    } else {
        state.engine.forget(mid, &scope).await.map_err(map_error)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_history(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<MemoryDto>>, (StatusCode, Json<ApiError>)> {
    let mid =
        MemoryId::parse(&id).map_err(|_| ApiError::validation(format!("invalid id '{id}'")))?;
    let scope = scope_from(None, None, &params);
    let chain = state.engine.history(mid, &scope).await.map_err(map_error)?;
    Ok(Json(chain.iter().map(MemoryDto::from_record).collect()))
}

async fn get_provenance(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ProvenanceDto>, (StatusCode, Json<ApiError>)> {
    let mid =
        MemoryId::parse(&id).map_err(|_| ApiError::validation(format!("invalid id '{id}'")))?;
    let scope = scope_from(None, None, &params);
    let record = state
        .engine
        .recall_exact(mid, &scope)
        .await
        .map_err(map_error)?
        .ok_or_else(|| ApiError::not_found(&id))?;
    Ok(Json(ProvenanceDto {
        id: record.id.to_string(),
        source: format!("{:?}", record.provenance.source).to_lowercase(),
        actor_id: record.provenance.actor_id.clone(),
        trace_id: record.provenance.trace_id.clone(),
        recorded_at: record.provenance.recorded_at.to_rfc3339(),
    }))
}

async fn recall(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<QueryBody>,
) -> Result<Json<Vec<ScoredMemoryDto>>, (StatusCode, Json<ApiError>)> {
    if body.query.trim().is_empty() {
        return Err(ApiError::validation("query must not be empty"));
    }
    let mut request =
        memory_retrieval::RecallRequest::new(body.query.clone()).with_budget(body.limit.max(1));
    if body.tenant_id.is_some() || body.user_id.is_some() {
        let mut b = MemoryScopeBuilder::new();
        if let Some(t) = &body.tenant_id {
            b = b.tenant(t.clone());
        }
        if let Some(u) = &body.user_id {
            b = b.user(u.clone());
        }
        request = request.with_scope(b.build());
    }

    use memory_domain::MemoryError;
    let ranked = state.engine.recall(&request).await.map_err(|e| match e {
        MemoryError::Unsupported(m) => ApiError::validation(m),
        other => map_error(other),
    })?;
    Ok(Json(
        ranked
            .iter()
            .map(|c| ScoredMemoryDto {
                memory: MemoryDto::from_record(&c.record),
                score: c.score,
            })
            .collect(),
    ))
}

async fn search(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<QueryBody>,
) -> Result<Json<Vec<MemoryDto>>, (StatusCode, Json<ApiError>)> {
    if body.query.trim().is_empty() {
        return Err(ApiError::validation("query must not be empty"));
    }
    let mut q = MemoryQuery::default()
        .with_text(body.query.clone())
        .with_limit(body.limit.max(1));
    if body.tenant_id.is_some() || body.user_id.is_some() {
        let mut b = MemoryScopeBuilder::new();
        if let Some(t) = &body.tenant_id {
            b = b.tenant(t.clone());
        }
        if let Some(u) = &body.user_id {
            b = b.user(u.clone());
        }
        q = q.with_scope(b.build());
    }
    let hits = state.engine.search(q).await.map_err(map_error)?;
    Ok(Json(hits.iter().map(MemoryDto::from_record).collect()))
}
