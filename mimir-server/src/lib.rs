pub mod error;
pub mod routes;
pub mod state;
pub mod types;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::ConnectInfo,
    http::StatusCode,
    middleware::from_fn,
    response::IntoResponse,
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

use mimir_core::config::Config;

use crate::routes::{
    chat_handler, chat_stream_handler, memory_handler, status_handler, stop_handler,
};
use crate::state::AppState;

/// Middleware guard that restricts access to loopback addresses.
async fn require_loopback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !addr.ip().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

/// Build the Axum router with all routes and middleware.
pub fn build_app(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:8080"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://127.0.0.1:8080"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://localhost:3000"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://127.0.0.1:3000"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://localhost:5173"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://127.0.0.1:5173"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
        ])
        .allow_methods([http::Method::GET, http::Method::POST])
        .allow_headers([http::header::CONTENT_TYPE]);

    Router::new()
        .route("/status", get(status_handler))
        .route("/memory", get(memory_handler))
        .route("/chat", post(chat_handler))
        .route("/chat/stream", post(chat_stream_handler))
        .route("/stop", post(stop_handler).layer(from_fn(require_loopback)))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors),
        )
        .with_state(state)
}

/// Start the Mimir HTTP server using the provided configuration.
///
/// Loads shared state from `config`, binds to `config.server.bind_addr`,
/// and runs until the process is terminated or a graceful shutdown is
/// triggered via the `/stop` endpoint.
pub async fn start_server(config: Config) -> anyhow::Result<()> {
    let bind_addr = config.server.bind_addr.clone();
    let state = Arc::new(AppState::from_config(config).await?);
    let mut shutdown_rx = state.shutdown_tx.subscribe();

    let app = build_app(state);

    let addr: SocketAddr = bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Mimir daemon listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use dashmap::DashMap;
    use tower::ServiceExt;

    use mimir_api_types::{ChatResponse, StatusResponse};
    use mimir_core::{
        config::PersonalityConfig,
        context::ContextManager,
        llm::types::{LlmError, StreamItem, Usage},
        llm::{LlmBackend, MockLlmClient},
        personality::Personality,
    };

    use crate::state::AppState;

    /// Build an `AppState` suitable for tests, using a temporary directory
    /// for the context database and memory.md.
    async fn test_state(llm: Arc<dyn LlmBackend>) -> (Arc<AppState>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("context.db");
        let memory_path = temp.path().join("memory.md");

        tokio::fs::write(&memory_path, "Test memory content")
            .await
            .unwrap();

        let context_manager = Arc::new(ContextManager::new(&db_path).await.unwrap());
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let state = Arc::new(AppState {
            llm_client: llm,
            context_manager,
            memory_path,
            personality: Personality::new(&PersonalityConfig::default()),
            session_locks: Arc::new(DashMap::new()),
            start_time: Instant::now(),
            endpoint: "http://localhost:8080".to_string(),
            model: "gpt-4o".to_string(),
            memory_limit: 10_000,
            shutdown_tx,
            model_override_cache: Arc::new(DashMap::new()),
        });

        (state, temp)
    }

    #[tokio::test]
    async fn test_status_returns_ok() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_chat_creates_session() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hello!", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat: ChatResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(!chat.session_id.is_empty());
        assert_eq!(chat.response, "Hello!");
    }

    #[tokio::test]
    async fn test_chat_stream_returns_ok() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("hi".to_string())),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            ct.starts_with("text/event-stream"),
            "expected SSE content-type, got: {}",
            ct
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("data: hi"), "expected text frame in SSE body");
        assert!(
            text.contains("event: usage"),
            "expected usage frame in SSE body"
        );
        assert!(
            text.contains("\n\n"),
            "expected SSE frames terminated with double newline"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_llm_error_sends_error_event() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("partial".to_string())),
                    Err(LlmError::Api {
                        status: 500,
                        body: "boom".to_string(),
                    }),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("partial"));
        assert!(text.contains("error"));
    }

    #[tokio::test]
    async fn test_chat_queue_full_returns_503() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_error(LlmError::QueueFull)
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(retry, "5");
    }

    #[tokio::test]
    async fn test_status_returns_queue_depths() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .user_queue_depth(2)
                .system_queue_depth(1)
                .worker_threads(4)
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: StatusResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.queue_depth_user, 2);
        assert_eq!(status.queue_depth_system, 1);
        assert_eq!(status.worker_threads, 4);
    }

    #[tokio::test]
    async fn test_chat_unknown_session_returns_404() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(
            &serde_json::json!({"session_id": "not-a-real-id", "message": "hello"}),
        )
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_memory_returns_content() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/memory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(text.contains("Test memory content"));
    }

    #[tokio::test]
    async fn test_stop_returns_ok() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stop")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_stop_rejects_non_loopback() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stop")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
