//! Rust client SDK for the Barq memory server.
//!
//! Identical concepts to the Python/Node/.NET SDKs: remember, recall,
//! search, update, forget, history. The transport is a trait so tests
//! (and exotic runtimes) inject their own HTTP layer; `reqwest` is the
//! default implementation.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client errors: transport failures and API error envelopes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// The HTTP exchange itself failed.
    #[error("transport: {0}")]
    Transport(String),
    /// The API answered with a non-success status.
    #[error("api ({status}): {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error envelope message.
        message: String,
    },
    /// The response body could not be decoded.
    #[error("decode: {0}")]
    Decode(String),
}

/// Result alias for SDK calls.
pub type SdkResult<T> = Result<T, ClientError>;

/// Minimal HTTP transport so the SDK is testable without sockets.
#[async_trait::async_trait]
pub trait HttpTransport: Send + Sync {
    /// Performs one JSON request; returns (status, body).
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> SdkResult<(u16, Value)>;
}

/// A canonical memory as seen on the wire (camelCase ids match the API).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemoryView {
    pub id: String,
    #[serde(rename = "type")]
    pub memory_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub status: String,
    pub version: u64,
    pub confidence: f32,
    pub importance: f32,
    pub created_at: String,
    pub updated_at: String,
}

/// A recall hit with its similarity score.
#[derive(Clone, Debug, Deserialize)]
pub struct ScoredMemoryView {
    #[serde(flatten)]
    pub memory: MemoryView,
    pub score: f32,
}

/// Options for [`MemoryClient::remember`].
#[derive(Clone, Debug, Default)]
pub struct RememberOptions {
    pub tenant_id: Option<String>,
    pub user_id: Option<String>,
    pub memory_type: Option<String>,
    pub confidence: Option<f32>,
}

/// The Barq memory client.
pub struct MemoryClient<T: HttpTransport> {
    transport: std::sync::Arc<T>,
}

impl<T: HttpTransport> MemoryClient<T> {
    /// Wraps a transport.
    pub fn new(transport: T) -> Self {
        Self {
            transport: std::sync::Arc::new(transport),
        }
    }

    fn check(status: u16, body: Value) -> SdkResult<()> {
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(ClientError::Api {
                status,
                message: body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string(),
            })
        }
    }

    fn decode<U: for<'de> Deserialize<'de>>(status: u16, body: Value) -> SdkResult<U> {
        Self::check(status, body.clone())?;
        serde_json::from_value(body).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Stores a new memory.
    pub async fn remember(
        &self,
        text: impl Into<String>,
        options: RememberOptions,
    ) -> SdkResult<MemoryView> {
        let mut body = serde_json::json!({ "text": text.into() });
        if let Some(t) = options.tenant_id {
            body["tenant_id"] = Value::String(t);
        }
        if let Some(u) = options.user_id {
            body["user_id"] = Value::String(u);
        }
        if let Some(ty) = options.memory_type {
            body["type"] = Value::String(ty);
        }
        if let Some(c) = options.confidence {
            body["confidence"] = serde_json::json!(c);
        }
        let (status, body) = self
            .transport
            .request("POST", "/v1/memories", Some(body))
            .await?;
        Self::decode(status, body)
    }

    /// Fetches one memory by id.
    pub async fn get(&self, id: &str) -> SdkResult<MemoryView> {
        let (status, body) = self
            .transport
            .request("GET", &format!("/v1/memories/{id}"), None)
            .await?;
        Self::decode(status, body)
    }

    /// Hybrid recall.
    pub async fn recall(
        &self,
        query: impl Into<String>,
        tenant_id: Option<&str>,
        limit: u32,
    ) -> SdkResult<Vec<ScoredMemoryView>> {
        let mut body = serde_json::json!({ "query": query.into(), "limit": limit.max(1) });
        if let Some(t) = tenant_id {
            body["tenant_id"] = Value::String(t.to_string());
        }
        let (status, body) = self
            .transport
            .request("POST", "/v1/recall", Some(body))
            .await?;
        Self::decode(status, body)
    }

    /// Keyword search.
    pub async fn search(
        &self,
        query: impl Into<String>,
        tenant_id: Option<&str>,
        limit: u32,
    ) -> SdkResult<Vec<MemoryView>> {
        let mut body = serde_json::json!({ "query": query.into(), "limit": limit.max(1) });
        if let Some(t) = tenant_id {
            body["tenant_id"] = Value::String(t.to_string());
        }
        let (status, body) = self
            .transport
            .request("POST", "/v1/search", Some(body))
            .await?;
        Self::decode(status, body)
    }

    /// Updates content by supersession; returns the successor.
    pub async fn update(&self, id: &str, new_text: impl Into<String>) -> SdkResult<MemoryView> {
        let body = serde_json::json!({ "text": new_text.into() });
        let (status, body) = self
            .transport
            .request("PATCH", &format!("/v1/memories/{id}"), Some(body))
            .await?;
        Self::decode(status, body)
    }

    /// Soft-deletes; hard=true purges physically.
    pub async fn forget(&self, id: &str, hard: bool) -> SdkResult<()> {
        let path = if hard {
            format!("/v1/memories/{id}?hard=true")
        } else {
            format!("/v1/memories/{id}")
        };
        let (status, body) = self.transport.request("DELETE", &path, None).await?;
        Self::check(status, body)
    }

    /// Supersession chain, oldest first.
    pub async fn history(&self, id: &str) -> SdkResult<Vec<MemoryView>> {
        let (status, body) = self
            .transport
            .request("GET", &format!("/v1/memories/{id}/history"), None)
            .await?;
        Self::decode(status, body)
    }
}

/// reqwest-backed transport for real deployments.
#[cfg(feature = "reqwest")]
pub struct ReqwestTransport {
    base: String,
    client: reqwest::Client,
}

#[cfg(feature = "reqwest")]
impl ReqwestTransport {
    /// Points at a running server, e.g. "http://127.0.0.1:8080".
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: {
                let b: String = base.into();
                b.trim_end_matches('/').to_string()
            },
            client: reqwest::Client::new(),
        }
    }
}

#[cfg(feature = "reqwest")]
#[async_trait::async_trait]
impl HttpTransport for ReqwestTransport {
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> SdkResult<(u16, Value)> {
        let url = format!("{}{}", self.base, path);
        let mut request = self.client.request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|e| ClientError::Transport(e.to_string()))?,
            &url,
        );
        if let Some(json) = body {
            request = request.json(&json);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).map_err(|e| ClientError::Decode(e.to_string()))?
        };
        Ok((status, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records requests and answers from a scripted map.
    struct FakeTransport {
        calls: Mutex<Vec<(String, String)>>,
        responses: std::collections::HashMap<(String, String), (u16, Value)>,
    }

    #[async_trait::async_trait]
    impl HttpTransport for FakeTransport {
        async fn request(
            &self,
            method: &str,
            path: &str,
            _body: Option<Value>,
        ) -> SdkResult<(u16, Value)> {
            self.calls
                .lock()
                .unwrap()
                .push((method.to_string(), path.to_string()));
            let key = (method.to_string(), path.to_string());
            self.responses
                .get(&key)
                .cloned()
                .ok_or_else(|| ClientError::Transport(format!("no scripted response for {key:?}")))
        }
    }

    fn record() -> Value {
        serde_json::json!({
            "id": "abc-123",
            "type": "semantic",
            "text": "Customer prefers email",
            "status": "active",
            "version": 1,
            "confidence": 0.9,
            "importance": 0.5,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
    }

    #[tokio::test]
    async fn remember_posts_and_decodes_the_record() {
        let transport = FakeTransport {
            calls: Mutex::new(Vec::new()),
            responses: [(("POST".into(), "/v1/memories".into()), (201, record()))]
                .into_iter()
                .collect(),
        };
        let client = MemoryClient::new(transport);
        let saved = client
            .remember(
                "Customer prefers email",
                RememberOptions {
                    tenant_id: Some("acme".into()),
                    confidence: Some(0.9),
                    ..Default::default()
                },
            )
            .await
            .expect("remember");
        assert_eq!(saved.id, "abc-123");
        assert_eq!(saved.memory_type, "semantic");
    }

    #[tokio::test]
    async fn api_errors_surface_as_typed_failures() {
        let transport = FakeTransport {
            calls: Mutex::new(Vec::new()),
            responses: [((
                "GET".into(),
                "/v1/memories/missing".into(),
            ), (
                404,
                serde_json::json!({ "error": "not_found", "message": "memory missing not found" }),
            ))]
            .into_iter()
            .collect(),
        };
        let client = MemoryClient::new(transport);
        match client.get("missing").await {
            Err(ClientError::Api { status, message }) => {
                assert_eq!(status, 404);
                assert!(message.contains("not found"));
            }
            other => panic!("expected api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lifecycle_calls_hit_the_expected_paths() {
        let transport = FakeTransport {
            calls: Mutex::new(Vec::new()),
            responses: [
                (
                    ("PATCH".into(), "/v1/memories/abc-123".into()),
                    (200, record()),
                ),
                (
                    ("DELETE".into(), "/v1/memories/abc-123".into()),
                    (204, Value::Null),
                ),
                (
                    ("GET".into(), "/v1/memories/abc-123/history".into()),
                    (200, serde_json::json!([record()])),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let client = MemoryClient::new(transport);
        client.update("abc-123", "new text").await.expect("update");
        client.forget("abc-123", false).await.expect("forget");
        let chain = client.history("abc-123").await.expect("history");
        assert_eq!(chain.len(), 1);
    }
}
