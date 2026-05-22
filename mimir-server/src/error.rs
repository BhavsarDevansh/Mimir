use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tracing::error;

/// Unified API error type for the HTTP server.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: String,
}

impl ApiError {
    fn new(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: code.into(),
        }
    }
}

/// Return a `BAD_REQUEST` response for invalid JSON.
pub fn json_rejection() -> Response {
    let body = Json(ApiError::new("invalid JSON body", "JSON_REJECTION"));
    (StatusCode::BAD_REQUEST, body).into_response()
}

/// Return a `NOT_FOUND` response when a session does not exist.
pub fn session_not_found() -> Response {
    let body = Json(ApiError::new("session not found", "SESSION_NOT_FOUND"));
    (StatusCode::NOT_FOUND, body).into_response()
}

/// Convert a context error into an HTTP response.
pub fn context_error(e: mimir_core::context::ContextError) -> Response {
    error!("context error: {e}");
    let body = Json(ApiError::new("internal server error", "CONTEXT_ERROR"));
    (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
}

/// Convert an LLM error into an HTTP response.
///
/// Queue-full errors return `503 Service Unavailable` with a `Retry-After: 5` header.
pub fn llm_error(e: mimir_core::llm::types::LlmError) -> Response {
    match &e {
        mimir_core::llm::types::LlmError::QueueFull => {
            let body = Json(ApiError::new("server busy, try again later", "QUEUE_FULL"));
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "5")],
                body,
            )
                .into_response()
        }
        _ => {
            error!("LLM error: {e}");
            let body = Json(ApiError::new("internal server error", "LLM_ERROR"));
            (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
        }
    }
}

/// Convert a memory I/O error into an HTTP response.
pub fn memory_error(e: anyhow::Error) -> Response {
    error!("memory error: {e}");
    let body = Json(ApiError::new("internal server error", "MEMORY_ERROR"));
    (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
}

/// Generic internal-error response.
pub fn internal(msg: impl Into<String>) -> Response {
    let body = Json(ApiError::new(msg, "INTERNAL_ERROR"));
    (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
}
