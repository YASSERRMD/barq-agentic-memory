//! Wire DTOs for the REST surface.
//!
//! Field names are camelCase on the wire; ids stay hyphenated UUID
//! strings. These types deliberately mirror the Python/Node bindings'
//! option shapes so SDKs map 1:1.

use serde::{Deserialize, Serialize};

/// Body for `POST /v1/memories`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateMemoryBody {
    pub text: String,
    #[serde(default, alias = "type")]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// Body for `PATCH /v1/memories/{id}`.
#[derive(Clone, Debug, Deserialize)]
pub struct UpdateMemoryBody {
    pub text: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub importance: Option<f32>,
}

/// Body for `POST /v1/recall` and `POST /v1/search`.
#[derive(Clone, Debug, Deserialize)]
pub struct QueryBody {
    pub query: String,
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    10
}

/// Canonical record on the wire.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemoryDto {
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

impl MemoryDto {
    pub fn from_record(r: &memory_domain::MemoryRecord) -> Self {
        Self {
            id: r.id.to_string(),
            memory_type: r.memory_type.as_str().to_string(),
            text: r.content.text.clone(),
            subject: r.subject.as_ref().map(|s| s.canonical_key()),
            status: r.status.to_string(),
            version: r.version,
            confidence: r.confidence,
            importance: r.importance,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        }
    }
}

/// Scored recall result.
#[derive(Clone, Debug, Serialize)]
pub struct ScoredMemoryDto {
    #[serde(flatten)]
    pub memory: MemoryDto,
    pub score: f32,
}

/// Provenance view for `GET /v1/memories/{id}/provenance`.
#[derive(Clone, Debug, Serialize)]
pub struct ProvenanceDto {
    pub id: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub recorded_at: String,
}

/// Error envelope: machine-readable kind plus a human message.
#[derive(Serialize)]
pub struct ApiError {
    pub error: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn validation(message: impl Into<String>) -> (axum::http::StatusCode, axum::Json<Self>) {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(Self {
                error: "validation",
                message: message.into(),
            }),
        )
    }

    pub fn not_found(id: &str) -> (axum::http::StatusCode, axum::Json<Self>) {
        (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(Self {
                error: "not_found",
                message: format!("memory {id} not found"),
            }),
        )
    }

    pub fn internal(message: impl Into<String>) -> (axum::http::StatusCode, axum::Json<Self>) {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(Self {
                error: "internal",
                message: message.into(),
            }),
        )
    }
}
