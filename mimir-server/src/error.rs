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

/// Map a connector runtime error onto an HTTP response (Phase 3 A2 / #203).
///
/// `UnsupportedAction` / `Config` / `Parse` → `400`, `NotAuthenticated` /
/// `Authentication` → `401`, `Network` → `502 Bad Gateway`, `BackendNotFound` →
/// `404`, and the remaining variants (`Io`, `Other`) → `500`. The error
/// detail is surfaced for client-actionable codes and masked for `500`.
pub fn connector_error(e: mimir_connectors::ConnectorError) -> Response {
    use mimir_connectors::ConnectorError;
    error!("connector error: {e}");
    let (status, code, detail): (StatusCode, &str, String) = match &e {
        ConnectorError::UnsupportedAction(_)
        | ConnectorError::Config(_)
        | ConnectorError::Parse(_) => (
            StatusCode::BAD_REQUEST,
            "CONNECTOR_BAD_REQUEST",
            e.to_string(),
        ),
        ConnectorError::NotAuthenticated => (
            StatusCode::UNAUTHORIZED,
            "CONNECTOR_UNAUTHORIZED",
            e.to_string(),
        ),
        // `Authentication` may embed provider-echoed request details (OAuth
        // token-endpoint failures); the full detail is logged above via
        // `error!`, but the client-facing message stays fixed so a provider
        // response can never leak credentials or tokens back to the caller.
        ConnectorError::Authentication(_) => (
            StatusCode::UNAUTHORIZED,
            "CONNECTOR_UNAUTHORIZED",
            "authentication failed".to_string(),
        ),
        ConnectorError::Network(_) => (StatusCode::BAD_GATEWAY, "CONNECTOR_NETWORK", e.to_string()),
        ConnectorError::BackendNotFound { .. } => (
            StatusCode::NOT_FOUND,
            "CONNECTOR_BACKEND_NOT_FOUND",
            e.to_string(),
        ),
        ConnectorError::BackendAlreadyRegistered { .. } => (
            StatusCode::CONFLICT,
            "CONNECTOR_BACKEND_CONFLICT",
            e.to_string(),
        ),
        ConnectorError::Io(_) | ConnectorError::Other(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "CONNECTOR_ERROR",
            "internal connector error".to_string(),
        ),
    };
    let body = Json(ApiError::new(detail, code));
    (status, body).into_response()
}

/// Map a connector-credential error onto an HTTP response (Phase 3 A2 / #203).
///
/// `InvalidSlug` and `Corrupt` → `400`; `InsecurePermissions` → `500` (the
/// store refuses to read a too-permissive file — surfaced as internal so the
/// detail does not leak a path); `Paths` / `Io` / `Serialize` → `500`.
pub fn secret_error(e: mimir_connectors::SecretError) -> Response {
    use mimir_connectors::SecretError;
    error!("secret store error: {e}");
    let (status, code, detail): (StatusCode, &str, String) = match &e {
        SecretError::InvalidSlug { .. } | SecretError::Corrupt { .. } => {
            (StatusCode::BAD_REQUEST, "SECRET_BAD_REQUEST", e.to_string())
        }
        SecretError::InsecurePermissions { .. }
        | SecretError::Paths(_)
        | SecretError::Io(_)
        | SecretError::Serialize(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "SECRET_ERROR",
            "internal secret store error".to_string(),
        ),
    };
    let body = Json(ApiError::new(detail, code));
    (status, body).into_response()
}

/// Map a supervisor lifecycle error (`start` / `pause` / `resume`) onto an HTTP
/// response (Phase 3 A2 / #203).
pub fn supervisor_error(e: mimir_connectors::SupervisorError) -> Response {
    use mimir_connectors::SupervisorError;
    match e {
        SupervisorError::Knowledge(ke) => knowledge_error(ke),
        SupervisorError::Connector(ce) => connector_error(ce),
        SupervisorError::Json(je) => bad_request(format!("invalid connector config: {je}")),
        SupervisorError::UnknownConnectorType { id, type_id } => bad_request(format!(
            "connector {id} has an unknown connector_type id {type_id}"
        )),
    }
}

/// Map a write-back dispatch error (`act`) onto an HTTP response
/// (Phase 3 A2 / #203).
pub fn act_error(e: mimir_connectors::ActError) -> Response {
    use mimir_connectors::ActError;
    match e {
        ActError::Knowledge(ke) => knowledge_error(ke),
        ActError::Connector(ce) => connector_error(ce),
        ActError::NotFound(id) => not_found(format!("connector not found: {id}")),
        ActError::UnknownType { id, type_id } => bad_request(format!(
            "connector {id} has an unknown connector_type id {type_id}"
        )),
    }
}

/// Map a manual-sync trigger error onto an HTTP response (Phase 3 A2 / #203).
pub fn trigger_error(e: mimir_connectors::TriggerError) -> Response {
    use mimir_connectors::TriggerError;
    match e {
        TriggerError::Knowledge(ke) => knowledge_error(ke),
        TriggerError::NotFound(id) => not_found(format!("connector not found: {id}")),
        TriggerError::NotFoundSlug(slug) => not_found(format!("connector not found: {slug}")),
        TriggerError::NotRunning { id, status } => {
            let detail = format!(
                "connector {id} is not running (status: {})",
                status
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            );
            let body = Json(ApiError::new(detail, "CONNECTOR_NOT_RUNNING"));
            (StatusCode::CONFLICT, body).into_response()
        }
        TriggerError::PushUnsupported { id } => {
            let body = Json(ApiError::new(
                format!("connector {id} runs in push mode; manual sync is not supported"),
                "CONNECTOR_PUSH_UNSUPPORTED",
            ));
            (StatusCode::CONFLICT, body).into_response()
        }
        TriggerError::RunnerDropped(id) => {
            error!("connector {id} runner dropped mid-sync");
            internal(format!("connector {id} sync did not complete"))
        }
    }
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
        | KnowledgeError::ConnectorNotFound(_)
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
    async fn knowledge_error_connector_not_found_returns_404_with_detail() {
        // A delete of an unknown connector must surface the not-found detail
        // (e.g. "Connector 7 not found"), not the generic "internal knowledge
        // graph error" mask.
        let resp = knowledge_error(mimir_knowledge::KnowledgeError::ConnectorNotFound(7));
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
        assert!(body["error"].as_str().unwrap().contains("7"));
        assert_ne!(body["error"], "internal knowledge graph error");
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

    // -- Connector error mappings (Phase 3 A2 / #203) --

    #[tokio::test]
    async fn connector_unsupported_action_returns_400() {
        let resp = connector_error(mimir_connectors::ConnectorError::UnsupportedAction(
            "bogus".to_string(),
        ));
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_BAD_REQUEST");
    }

    #[tokio::test]
    async fn connector_not_authenticated_returns_401() {
        let resp = connector_error(mimir_connectors::ConnectorError::NotAuthenticated);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_UNAUTHORIZED");
    }

    /// `Authentication` may embed provider-echoed request details (OAuth
    /// token-endpoint failures); the client-facing body must stay fixed so a
    /// provider response can never leak credentials back to the caller.
    #[tokio::test]
    async fn connector_authentication_masks_detail() {
        let resp = connector_error(mimir_connectors::ConnectorError::Authentication(
            "refresh failed: token=secret-ish&client_id=abc".into(),
        ));
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_UNAUTHORIZED");
        assert!(!body["error"].as_str().unwrap().contains("secret-ish"));
        assert_eq!(body["error"], "authentication failed");
    }

    #[tokio::test]
    async fn connector_network_returns_502() {
        let resp = connector_error(mimir_connectors::ConnectorError::Network("down".into()));
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_NETWORK");
    }

    #[tokio::test]
    async fn connector_other_masks_detail() {
        let resp = connector_error(mimir_connectors::ConnectorError::Other("secret-ish".into()));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_ERROR");
        assert!(!body["error"].as_str().unwrap().contains("secret-ish"));
    }

    #[tokio::test]
    async fn secret_invalid_slug_returns_400() {
        let resp = secret_error(mimir_connectors::SecretError::InvalidSlug {
            slug: "a b".to_string(),
            reason: "space".to_string(),
        });
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "SECRET_BAD_REQUEST");
    }

    #[tokio::test]
    async fn act_error_not_found_returns_404() {
        let resp = act_error(mimir_connectors::ActError::NotFound(7));
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
    }

    #[tokio::test]
    async fn trigger_not_running_returns_409() {
        let resp = trigger_error(mimir_connectors::TriggerError::NotRunning {
            id: 3,
            status: Some(mimir_knowledge::models::enums::ConnectorStatus::Paused),
        });
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_NOT_RUNNING");
    }

    #[tokio::test]
    async fn trigger_push_unsupported_returns_409() {
        let resp = trigger_error(mimir_connectors::TriggerError::PushUnsupported { id: 5 });
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "CONNECTOR_PUSH_UNSUPPORTED");
    }

    #[tokio::test]
    async fn supervisor_unknown_connector_type_returns_400() {
        let resp = supervisor_error(mimir_connectors::SupervisorError::UnknownConnectorType {
            id: 9,
            type_id: 99,
        });
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["code"], "BAD_REQUEST");
    }
}
