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
use mimir_core::llm::{LlmBackend, LlmClient};

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

/// Combined shutdown signal that races Ctrl-C, SIGTERM (Unix), and the
/// `/stop` endpoint watch channel.
async fn shutdown_signal(mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
            _ = shutdown_rx.changed() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = shutdown_rx.changed() => {},
        }
    }
}

/// Start the Mimir HTTP server using the provided configuration.
///
/// Loads shared state from `config`, binds to `config.server.bind_addr`,
/// and runs until the process is terminated or a graceful shutdown is
/// triggered via the `/stop` endpoint, Ctrl-C, or SIGTERM.
///
/// If the server does not shut down gracefully within 30 seconds, it is
/// forcefully aborted so that resource cleanup can still run.
pub async fn start_server(config: Config) -> anyhow::Result<()> {
    let llm_client: Arc<dyn LlmBackend> = Arc::new(LlmClient::new(config.llm.clone()).await);
    start_server_with_llm(config, llm_client).await
}

/// Start the Mimir HTTP server with an injected LLM backend.
///
/// This is the same as [`start_server`], but allows tests (and future
/// embedders) to supply a custom [`LlmBackend`] implementation without
/// relying on sentinel strings or config hacks.
pub async fn start_server_with_llm(
    config: Config,
    llm_client: Arc<dyn LlmBackend>,
) -> anyhow::Result<()> {
    let bind_addr = config.server.bind_addr.clone();
    let addr: SocketAddr = bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    start_server_with_llm_and_listener(config, llm_client, listener).await
}

/// Start the Mimir HTTP server with an injected LLM backend and a pre-bound listener.
///
/// This is the same as [`start_server_with_llm`], but allows tests to supply
/// a pre-bound [`TcpListener`] so the bound port is known before the server
/// starts accepting connections.
pub async fn start_server_with_llm_and_listener(
    config: Config,
    llm_client: Arc<dyn LlmBackend>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let state = Arc::new(AppState::from_config_with_llm(config, llm_client).await?);
    let shutdown_rx = state.shutdown_tx.subscribe();

    let app = build_app(Arc::clone(&state));

    info!("Mimir daemon listening on {}", listener.local_addr()?);

    let server_fut = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_rx));

    match tokio::time::timeout(std::time::Duration::from_secs(30), server_fut).await {
        Ok(result) => {
            result?;
            info!("Server shut down gracefully.");
        }
        Err(_) => {
            tracing::warn!("Graceful shutdown timed out after 30s; forcing exit.");
        }
    }

    state.shutdown().await;
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
        llm::types::{FunctionCall, LlmError, Message, StreamItem, ToolCall, Usage},
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
            tool_registry: Arc::new(mimir_core::tools::ToolRegistry::with_builtins()),
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
    async fn test_chat_forwards_tools_to_llm() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hello!", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
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

        // Built-in tools should have been forwarded.
        let tools = mock.chat_tools();
        assert_eq!(tools.len(), 1);
        let forwarded = tools[0].as_ref().expect("tools should be forwarded");
        assert!(!forwarded.is_empty(), "at least one tool should be present");
        let names: Vec<String> = forwarded
            .iter()
            .filter_map(|t| t.get("function")?.get("name")?.as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(names.contains(&"get_current_time".to_string()));
        assert!(names.contains(&"echo".to_string()));
    }

    #[tokio::test]
    async fn test_chat_executes_tool_calls_and_returns_final_response() {
        let tool_call = ToolCall {
            index: 0,
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_current_time".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let first_response = Message {
            role: "assistant".to_string(),
            content: "".to_string(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(first_response, Usage::default())
                .push_chat("The current time is now.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
        let app = super::build_app(state);

        let body =
            serde_json::to_string(&serde_json::json!({"message": "What time is it?"})).unwrap();
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
        assert_eq!(chat.response, "The current time is now.");

        // Should have made two LLM calls: one for the tool call, one for the final answer.
        let calls = mock.chat_calls();
        assert_eq!(
            calls.len(),
            2,
            "expected two LLM calls (tool request + follow-up)"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_forwards_tools_to_llm() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("Hello!".to_string())),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
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

        let tools = mock.stream_tools();
        assert_eq!(tools.len(), 1);
        let forwarded = tools[0].as_ref().expect("tools should be forwarded");
        assert!(!forwarded.is_empty(), "at least one tool should be present");
        let names: Vec<String> = forwarded
            .iter()
            .filter_map(|t| t.get("function")?.get("name")?.as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(names.contains(&"get_current_time".to_string()));
        assert!(names.contains(&"echo".to_string()));
    }

    #[tokio::test]
    async fn test_chat_stream_executes_tool_calls_and_returns_final_response() {
        let tool_call_delta = ToolCall {
            index: 0,
            id: "call_456".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_current_time".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::ToolCalls(vec![tool_call_delta])),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .push_chat("The current time is now.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
        let app = super::build_app(state);

        let body =
            serde_json::to_string(&serde_json::json!({"message": "What time is it?"})).unwrap();
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
        let text = String::from_utf8_lossy(&bytes);
        // The follow-up text should be streamed as a data event.
        assert!(
            text.contains("The current time is now."),
            "expected follow-up text in SSE stream, got: {}",
            text
        );

        // The follow-up call should have been made via the non-streaming chat path.
        let calls = mock.chat_calls();
        assert_eq!(
            calls.len(),
            1,
            "expected one follow-up LLM call after tool execution"
        );
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

    #[tokio::test]
    async fn test_server_exits_after_stop() {
        use mimir_core::config::Config;

        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("context.db");
        let memory_path = temp.path().join("memory.md");
        tokio::fs::write(&memory_path, "Test memory content")
            .await
            .unwrap();

        // Find an available port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let mut config = Config::default();
        config.llm.endpoint = "http://127.0.0.1:1".to_string();
        config.llm.api_key = "test".to_string();
        config.llm.model = "gpt-4o".to_string();
        config.llm.max_tokens = Some(10);
        config.llm.temperature = 0.0;
        config.server.bind_addr = addr.to_string();
        config.memory.char_limit = 10_000;
        config.context.db_path = Some(db_path);

        let handle = tokio::spawn(async move { super::start_server(config).await });

        // Poll until the server accepts a TCP connection (up to 5 s).
        let poll_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.await.unwrap();
                panic!("server exited early: {:?}", result);
            }
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "server did not become reachable within 5 seconds");

        // Send the stop request.
        let client = reqwest::Client::new();
        let res = client
            .post(format!("http://{}/stop", addr))
            .send()
            .await
            .unwrap();
        let status = res.status();
        let body = res.text().await.unwrap();
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "unexpected status: {} body: {}",
            status,
            body
        );

        // The server should exit within 5 seconds.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "server did not exit within 5 seconds");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }
}
