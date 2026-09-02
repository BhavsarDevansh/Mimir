use futures::StreamExt;
use mimir_core::{
    config::LlmConfig,
    llm::LlmClient,
    llm::client::RetryConfig,
    llm::types::{LlmError, Message, StreamItem},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

/// Build an `LlmClient` pointed at the given wiremock base URL.
async fn test_client(base_url: String) -> LlmClient {
    let config = LlmConfig {
        endpoint: base_url,
        api_key: "test-key".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.0,
    };
    LlmClient::new_with_retry_config(
        config,
        RetryConfig {
            max_attempts: 2,
            base_backoff: std::time::Duration::ZERO,
            max_backoff: std::time::Duration::ZERO,
        },
    )
    .await
    .expect("LLM client must build in tests")
}

#[tokio::test]
async fn test_retry_on_429() {
    let server = MockServer::start().await;

    // First request returns 429; second returns 200.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    let ok_body = r#"{"id":"1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ok_body))
        .expect(1)
        .mount(&server)
        .await;

    let client = test_client(server.uri()).await;
    let result = client.chat(vec![Message::user("hello")], None).await;

    assert!(result.is_ok());
    let (text, usage) = result.unwrap();
    assert_eq!(text, "OK");
    assert_eq!(usage.total_tokens, 2);
}

#[tokio::test]
async fn test_no_retry_on_400() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1)
        .mount(&server)
        .await;

    let client = test_client(server.uri()).await;
    let result = client.chat(vec![Message::user("hello")], None).await;

    assert!(result.is_err());
    match result {
        Err(LlmError::Api { status, body }) => {
            assert_eq!(status, 400);
            assert_eq!(body, "bad request");
        }
        other => panic!("expected Api error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_sse_stream_parsing() {
    let server = MockServer::start().await;

    let text_sse = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

"#;
    let usage_sse = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}

"#;
    let sse_body = format!("{}{}", text_sse, usage_sse);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = test_client(server.uri()).await;
    let mut stream = client
        .chat_stream_with_usage(vec![Message::user("hello")], None)
        .await
        .unwrap();

    let item1 = stream.next().await.unwrap().unwrap();
    assert!(matches!(item1, StreamItem::Text(t) if t == "Hello"));

    let item2 = stream.next().await.unwrap().unwrap();
    assert!(matches!(item2, StreamItem::Usage(u) if u.total_tokens == 4));

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_connection_failure() {
    // Point at a non-listening port to simulate connection failure.
    let client = test_client("http://127.0.0.1:1".to_string()).await;
    let result = client.chat(vec![Message::user("hello")], None).await;

    assert!(result.is_err());
    match result {
        Err(LlmError::Network(_)) | Err(LlmError::RetryExhausted { .. }) => {}
        other => panic!("expected Network or RetryExhausted error, got {:?}", other),
    }
}
