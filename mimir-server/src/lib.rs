pub mod error;
pub mod routes;
pub mod state;
pub mod types;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

use mimir_core::config::Config;

use crate::routes::{chat_handler, chat_stream_handler, memory_handler, status_handler};
use crate::state::AppState;

/// Default bind address for the Mimir HTTP server.
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";

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
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors),
        )
        .with_state(state)
}

/// Start the Mimir HTTP server on the given bind address.
///
/// Parses `bind_addr` as a `SocketAddr` and runs until the process is terminated.
pub async fn start_server(bind_addr: &str) -> anyhow::Result<()> {
    let config = Config::load(None)?;
    let state = Arc::new(AppState::from_config(config).await?);

    let app = build_app(state);

    let addr: SocketAddr = bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("mimir-server listening on {}", bind_addr);

    axum::serve(listener, app).await?;
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

    use mimir_core::{
        config::LlmConfig, context::ContextManager, llm::LlmClient, personality::Personality,
    };

    use crate::state::AppState;
    use crate::types::ChatResponse;

    /// Build an `AppState` suitable for tests, using a temporary directory
    /// for the context database and memory.md.
    async fn test_state(endpoint: String) -> (Arc<AppState>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("context.db");
        let memory_path = temp.path().join("memory.md");

        tokio::fs::write(&memory_path, "Test memory content")
            .await
            .unwrap();

        let llm_config = LlmConfig {
            endpoint,
            api_key: "test".to_string(),
            model: "gpt-4o".to_string(),
            max_tokens: Some(10),
            temperature: 0.0,
        };
        let llm_client = Arc::new(LlmClient::new(llm_config));

        let context_manager = Arc::new(ContextManager::new(&db_path).await.unwrap());

        let state = Arc::new(AppState {
            llm_client,
            context_manager,
            memory_path,
            personality: Personality::new(&mimir_core::config::PersonalityConfig::default()),
            session_locks: Arc::new(DashMap::new()),
            start_time: Instant::now(),
        });

        (state, temp)
    }

    /// Spawn a minimal HTTP server that returns a valid non-streaming chat completion.
    async fn mock_llm_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let body = r#"{"id":"1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"Hello!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ =
                        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
                });
            }
        });

        format!("http://{}/v1", addr)
    }

    #[tokio::test]
    async fn test_status_returns_ok() {
        let (state, _temp) = test_state("http://127.0.0.1:1".to_string()).await;
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
        let endpoint = mock_llm_server().await;
        let (state, _temp) = test_state(endpoint).await;
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

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_resp: ChatResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!chat_resp.session_id.is_empty());
        assert_eq!(chat_resp.response, "Hello!");
    }

    #[tokio::test]
    async fn test_chat_stream_returns_sse() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let sse_body = format!(
                        "data: {}\n\n",
                        r#"{"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                        sse_body.len(),
                        sse_body
                    );
                    let _ =
                        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
                });
            }
        });

        let endpoint = format!("http://{}/v1", addr);
        let (state, _temp) = test_state(endpoint).await;
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
        assert!(ct.contains("text/event-stream"));
    }

    #[tokio::test]
    async fn test_chat_unknown_session_returns_404() {
        let (state, _temp) = test_state("http://127.0.0.1:1".to_string()).await;
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
        let (state, _temp) = test_state("http://127.0.0.1:1".to_string()).await;
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
}
