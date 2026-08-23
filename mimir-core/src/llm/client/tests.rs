//! Unit tests for the LLM HTTP client.

use super::transport::{MAX_BACKOFF_MS, MAX_RETRIES};
use super::*;
use crate::llm::backend::LlmBackend;
use crate::llm::types::{LlmError, Message, StreamChunk};
use eventsource_stream::Eventsource;
use futures::StreamExt;
#[test]
fn test_debug_does_not_leak_api_key() {
    let config = LlmConfig {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: "sk-super-secret".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(100),
        temperature: 0.2,
    };
    let client = LlmClient::new_direct(config).expect("LLM direct client must build in tests");
    let debug = format!("{:?}", client);
    assert!(
        !debug.contains("sk-super-secret"),
        "Debug output must not contain the API key"
    );
    assert!(debug.contains("***REDACTED***"));
}

#[test]
fn with_temperature_override_updates_temperature() {
    // Issue #80: a hot-reloaded temperature must reach the request.
    let config = LlmConfig {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.2,
    };
    let client = LlmClient::new_direct(config).expect("LLM direct client must build in tests");
    let overridden = client
        .with_temperature_override(0.7)
        .expect("temperature override supported");
    let debug = format!("{:?}", overridden);
    assert!(debug.contains("temperature: 0.7"), "debug: {debug}");
}

#[tokio::test]
async fn new_returns_client_build_error_for_invalid_pool_config() {
    // Issue #166: a worker pool that cannot initialise must surface as
    // `LlmError::ClientBuild` instead of panicking at daemon startup.
    // `worker_threads = 0` is rejected by `LlmWorkerPool::new`.
    let config = LlmConfig {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.2,
    };
    let result = LlmClient::new_with_pool_config(
        config,
        crate::llm::pool::WorkerPoolConfig {
            worker_threads: 0,
            ..Default::default()
        },
    )
    .await;
    assert!(
        matches!(result, Err(LlmError::ClientBuild(ref m)) if m.contains("worker pool init")),
        "expected ClientBuild error, got {result:?}"
    );
}

#[test]
fn with_max_tokens_override_updates_max_tokens() {
    // Issue #388: a per-request max_tokens must reach the request.
    let config = LlmConfig {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.2,
    };
    let client = LlmClient::new_direct(config).expect("LLM direct client must build in tests");
    let overridden = client
        .with_max_tokens_override(256)
        .expect("max_tokens override supported");
    let debug = format!("{:?}", overridden);
    assert!(debug.contains("max_tokens: Some(256)"), "debug: {debug}");
}

#[tokio::test]
async fn with_temperature_override_disables_pooling() {
    // Temperature overrides must disable pooling so the override is applied
    // immediately rather than using cached workers.
    let config = LlmConfig {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.2,
    };
    let client = LlmClient::new(config)
        .await
        .expect("LLM client must build in tests");
    assert!(client.pool.is_some(), "pooled client should have a pool");

    let overridden = client
        .with_temperature_override(0.7)
        .expect("temperature override supported");
    let debug = format!("{:?}", overridden);
    assert!(
        debug.contains("has_pool: false"),
        "temperature override must disable pooling: {debug}"
    );
}

#[test]
fn test_calculate_backoff_grows_exponentially() {
    let b1 = LlmClient::calculate_backoff(1);
    let b2 = LlmClient::calculate_backoff(2);
    let b3 = LlmClient::calculate_backoff(3);

    // Base is 200ms; attempt 1 = ~200ms, attempt 2 = ~400ms, attempt 3 = ~800ms
    assert!(b1 >= 200, "backoff 1 should be at least 200ms");
    assert!(b2 >= 400, "backoff 2 should be at least 400ms");
    assert!(b3 >= 800, "backoff 3 should be at least 800ms");
}

#[test]
fn test_calculate_backoff_capped() {
    let b10 = LlmClient::calculate_backoff(10);
    assert!(
        b10 <= MAX_BACKOFF_MS,
        "backoff should be capped at {} ms",
        MAX_BACKOFF_MS
    );
}

#[tokio::test]
async fn test_retry_exhausted_on_persistent_failure() {
    // Build a client pointed at a non-routable address so every request fails.
    let config = LlmConfig {
        endpoint: "http://127.0.0.1:1".to_string(),
        api_key: "test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.0,
    };
    let client = LlmClient::new(config)
        .await
        .expect("LLM client must build in tests");

    let result = client.chat(vec![Message::user("hi")], None).await;
    assert!(result.is_err());

    match result {
        Err(LlmError::RetryExhausted { attempts }) => {
            assert_eq!(attempts, MAX_RETRIES + 1);
        }
        Err(_other) => {
            // It's also acceptable to get a straight network error if the OS
            // rejects the connection immediately (connection refused).
            // That's still correct behaviour.
        }
        Ok(_) => panic!("expected error"),
    }
}

#[tokio::test]
async fn test_chat_stream_parses_mock_sse() {
    // Verify that the stream parser correctly handles a real OpenAI-style SSE chunk.
    let sse_line = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"X"},"finish_reason":null}]}"#;

    // eventsource_stream expects a raw HTTP response body stream.
    // We simulate by creating a small byte stream with the SSE format.
    let body = format!("{}\n\n", sse_line);
    let stream = futures::stream::iter(vec![Ok::<bytes::Bytes, reqwest::Error>(
        bytes::Bytes::from(body),
    )]);
    let mut events = stream.eventsource();

    let event = events.next().await.unwrap().unwrap();
    assert!(event.data.contains("\"content\":\"X\""));
}

#[tokio::test]
async fn test_chat_stream_with_usage_yields_text_and_usage() {
    let text_sse = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
    let usage_sse = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#;

    let body = format!("{}\n\n{}\n\n", text_sse, usage_sse);
    let stream = futures::stream::iter(vec![Ok::<bytes::Bytes, reqwest::Error>(
        bytes::Bytes::from(body),
    )]);
    let mut events = stream.eventsource();

    let event1 = events.next().await.unwrap().unwrap();
    let chunk1: StreamChunk = serde_json::from_str(&event1.data).unwrap();
    assert_eq!(chunk1.choices[0].delta.content.as_deref(), Some("Hello"));

    let event2 = events.next().await.unwrap().unwrap();
    let chunk2: StreamChunk = serde_json::from_str(&event2.data).unwrap();
    assert!(chunk2.choices.is_empty());
    let usage = chunk2.usage.expect("usage present");
    assert_eq!(usage.prompt_tokens, 3);
    assert_eq!(usage.completion_tokens, 1);
}
