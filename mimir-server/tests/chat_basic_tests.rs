mod common;
use common::*;

#[tokio::test]
async fn test_chat_creates_session() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Hello!", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
    let response = app
        .oneshot(
            authed_request()
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
    assert!(chat.session_id > 0);
    assert_eq!(chat.response, "Hello!");
}
// Issues #137/#386: learning is hook-driven via `remember.chat`. A chitchat
// turn enqueues the hook, but the hook is idle-gated (cooldown + LLM-pool
// idle), so immediately after the response the mock must have recorded
// exactly one LLM call (the main chat completion) and no extraction call
// yet. The unconditional Librarian has been retired.
#[tokio::test]
async fn test_chitchat_does_not_trigger_background_learning() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Hi there! How can I help?", Usage::default())
            .build(),
    );
    let mut config = Config::default();
    config.identity.name = "devansh".to_string();
    let (state, _temp) = test_state_with_config(mock.clone(), config).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/chat")
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    assert_eq!(
        mock.chat_calls().len(),
        1,
        "chitchat must not trigger a background extraction LLM call"
    );
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
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
    let response = app
        .oneshot(
            authed_request()
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
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
    let response = app
        .oneshot(
            authed_request()
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
    assert!(
        text.contains("event: error"),
        "a failed stream must emit an error event: {text:?}"
    );
    assert!(
        text.contains("API error 500: boom"),
        "the error event must surface the LLM failure detail: {text:?}"
    );
}
#[tokio::test]
async fn test_chat_queue_full_returns_503() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(LlmError::QueueFull)
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
    let response = app
        .oneshot(
            authed_request()
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
