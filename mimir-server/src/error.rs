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

/// Return a `NOT_FOUND` response.
pub fn not_found(msg: impl Into<String>) -> Response {
    let body = Json(ApiError::new(msg, "NOT_FOUND"));
    (StatusCode::NOT_FOUND, body).into_response()
}
/// Return a `BAD_REQUEST` response.
pub fn bad_request(msg: impl Into<String>) -> Response {
    let body = Json(ApiError::new(msg, "BAD_REQUEST"));
    (StatusCode::BAD_REQUEST, body).into_response()
}

/// Convert a knowledge graph error into an HTTP response.
pub fn knowledge_error(e: mimir_knowledge::KnowledgeError) -> Response {
    use mimir_knowledge::KnowledgeError;
    error!("knowledge graph error: {e}");
    let (status, code) = match &e {
        KnowledgeError::Validation(_)
        | KnowledgeError::DuplicateEntity
        | KnowledgeError::DuplicatePreference => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
        KnowledgeError::EntityNotFound(_)
        | KnowledgeError::FactNotFound(_)
        | KnowledgeError::CategoryNotFound(_)
        | KnowledgeError::ConnectorNotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
        KnowledgeError::ConnectorSlugConflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "KG_ERROR"),
    };
    let message = match &e {
        KnowledgeError::Validation(_)
        | KnowledgeError::DuplicateEntity
        | KnowledgeError::DuplicatePreference
        | KnowledgeError::EntityNotFound(_)
        | KnowledgeError::FactNotFound(_)
        | KnowledgeError::CategoryNotFound(_)
        | KnowledgeError::ConnectorSlugConflict(_) => e.to_string(),
        _ => "internal knowledge graph error".to_string(),
    };
    let body = Json(ApiError::new(message, code));
    (status, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use serde_json::Value;

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    }

    #[tokio::test]
    async fn json_rejection_returns_400_with_code() {
        let resp = json_rejection();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "JSON_REJECTION");
        assert_eq!(body["error"], "invalid JSON body");
    }

    #[tokio::test]
    async fn session_not_found_returns_404() {
        let resp = session_not_found();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "SESSION_NOT_FOUND");
    }

    #[tokio::test]
    async fn context_error_returns_500_and_masks_detail() {
        let resp = context_error(mimir_core::context::ContextError::SessionNotFound(
            "internal-id".to_string(),
        ));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONTEXT_ERROR");
        // Internal detail must not leak to the client.
        assert_eq!(body["error"], "internal server error");
        assert!(!body["error"].as_str().unwrap().contains("internal-id"));
    }

    #[tokio::test]
    async fn llm_error_queue_full_returns_503_with_retry_after() {
        let resp = llm_error(mimir_core::llm::types::LlmError::QueueFull);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "5");
        let body = body_json(resp).await;
        assert_eq!(body["code"], "QUEUE_FULL");
    }

    #[tokio::test]
    async fn llm_error_generic_returns_500_without_detail() {
        let resp = llm_error(mimir_core::llm::types::LlmError::StreamError(
            "upstream detail".to_string(),
        ));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "LLM_ERROR");
        assert!(!body["error"].as_str().unwrap().contains("upstream detail"));
    }

    #[tokio::test]
    async fn memory_error_returns_500_and_masks_detail() {
        let resp = memory_error(anyhow::anyhow!("disk on fire"));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "MEMORY_ERROR");
        assert!(!body["error"].as_str().unwrap().contains("disk on fire"));
    }

    #[tokio::test]
    async fn internal_returns_500_with_message_and_code() {
        let resp = internal("something broke");
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "INTERNAL_ERROR");
        assert_eq!(body["error"], "something broke");
    }

    #[tokio::test]
    async fn not_found_returns_404_with_message() {
        let resp = not_found("widget missing");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
        assert_eq!(body["error"], "widget missing");
    }

    #[tokio::test]
    async fn knowledge_error_validation_returns_400_with_detail() {
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::Validation(
            "bad input".to_string(),
        ));
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "VALIDATION_ERROR");
        assert_eq!(body["error"], "Validation error: bad input");
    }

    #[tokio::test]
    async fn knowledge_error_duplicate_entity_returns_400() {
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::DuplicateEntity);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "VALIDATION_ERROR");
    }

    #[tokio::test]
    async fn knowledge_error_entity_not_found_returns_404_with_detail() {
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::EntityNotFound(42));
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
        assert!(body["error"].as_str().unwrap().contains("42"));
    }

    #[tokio::test]
    async fn knowledge_error_fact_not_found_returns_404() {
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::FactNotFound(7));
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
        assert!(body["error"].as_str().unwrap().contains("7"));
    }

    #[tokio::test]
    async fn knowledge_error_category_not_found_returns_404() {
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::CategoryNotFound(3));
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
    }

    #[tokio::test]
    async fn knowledge_error_connector_slug_conflict_returns_409() {
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::ConnectorSlugConflict(
            "personal".to_string(),
        ));
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONFLICT");
        assert!(body["error"].as_str().unwrap().contains("personal"));
    }

    #[tokio::test]
    async fn knowledge_error_internal_variant_masks_detail() {
        // Internal variants (e.g. NotYetImplemented) must NOT leak their detail.
        let err = mimir_knowledge::KnowledgeError::NotYetImplemented;
        let resp = knowledge_error(err);
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "KG_ERROR");
        assert_eq!(body["error"], "internal knowledge graph error");
    }

    #[tokio::test]
    async fn api_error_serializes_error_and_code_fields() {
        let err = ApiError::new("msg", "CODE");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["error"], "msg");
        assert_eq!(json["code"], "CODE");
    }
}
