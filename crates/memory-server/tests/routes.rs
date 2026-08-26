//! Hermetic route tests: in-memory engine + tower oneshot requests.
//! No sockets, no servers — the full HTTP surface exercised directly.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use memory_core::MemoryEngine;
use memory_domain::config::{EmbeddingConfig, EngineConfig, VectorStoreConfig};
use memory_server::{ServerState, router};
use serde_json::Value;
use tower::util::ServiceExt;

async fn app() -> axum::Router {
    let engine = MemoryEngine::from_config(EngineConfig {
        vector: Some(VectorStoreConfig::InMemory),
        embedding: Some(EmbeddingConfig::Hashing { dimensions: 128 }),
        ..EngineConfig::default()
    })
    .await
    .expect("engine");
    router(ServerState::new(engine))
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(json.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, json)
}

#[tokio::test]
async fn lifecycle_over_http_roundtrips() {
    let app = app().await;

    // Create.
    let (status, created) = call(
        &app,
        "POST",
        "/v1/memories",
        Some(serde_json::json!({
            "text": "Customer prefers email contact",
            "tenant_id": "acme",
            "user_id": "u-1",
            "confidence": 0.9,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["type"], "semantic");

    // Read (scoped).
    let (status, got) = call(
        &app,
        "GET",
        &format!("/v1/memories/{id}?tenant_id=acme"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["text"], "Customer prefers email contact");

    // Read under the wrong tenant looks like absence.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/v1/memories/{id}?tenant_id=globex"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Patch creates a successor.
    let (status, patched) = call(
        &app,
        "PATCH",
        &format!("/v1/memories/{id}?tenant_id=acme"),
        Some(serde_json::json!({ "text": "Customer prefers SMS now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    let new_id = patched["id"].as_str().expect("successor id").to_string();
    assert_ne!(new_id, id);

    // History reports both generations.
    let (status, chain) = call(&app, "GET", &format!("/v1/memories/{new_id}/history"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(chain.as_array().map(Vec::len), Some(2), "{chain}");

    // Provenance answers with the source trail.
    let (status, prov) = call(
        &app,
        "GET",
        &format!("/v1/memories/{new_id}/provenance"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(prov["source"].is_string());

    // Delete tombstones.
    let (status, _) = call(&app, "DELETE", &format!("/v1/memories/{new_id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", &format!("/v1/memories/{new_id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recall_and_search_endpoints() {
    let app = app().await;
    let (_, a) = call(
        &app,
        "POST",
        "/v1/memories",
        Some(serde_json::json!({ "text": "Project Atlas uses PostgreSQL", "tenant_id": "acme" })),
    )
    .await;
    let (_, _) = call(
        &app,
        "POST",
        "/v1/memories",
        Some(serde_json::json!({ "text": "Kitchen fridge needs restocking" })),
    )
    .await;
    assert!(a["id"].is_string());

    // Hybrid recall ranks the atlas fact first.
    let (status, hits) = call(
        &app,
        "POST",
        "/v1/recall",
        Some(serde_json::json!({ "query": "which database does atlas use", "limit": 5 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hits}");
    let arr = hits.as_array().expect("array");
    assert!(!arr.is_empty());
    assert_eq!(arr[0]["text"], "Project Atlas uses PostgreSQL");
    assert!(arr[0]["score"].as_f64().unwrap() > 0.0);

    // Search filters by words.
    let (status, found) = call(
        &app,
        "POST",
        "/v1/search",
        Some(serde_json::json!({ "query": "atlas postgresql", "tenant_id": "acme" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found.as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn validation_failures_are_400s_not_500s() {
    let app = app().await;

    let (status, body) = call(
        &app,
        "POST",
        "/v1/memories",
        Some(serde_json::json!({ "text": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "validation");

    let (status, body) = call(
        &app,
        "POST",
        "/v1/memories",
        Some(serde_json::json!({ "text": "x", "type": "quantum" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("quantum"));

    let (status, _) = call(&app, "GET", "/v1/memories/not-a-uuid", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &app,
        "POST",
        "/v1/recall",
        Some(serde_json::json!({ "query": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn healthz_is_alive() {
    let app = app().await;
    let (status, _) = call(&app, "GET", "/healthz", None).await;
    assert_eq!(status, StatusCode::OK);
}
