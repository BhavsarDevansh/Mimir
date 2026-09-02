mod common;
use common::*;

#[tokio::test]
async fn test_chat_unknown_session_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let body =
        serde_json::to_string(&serde_json::json!({"session_id": 999999, "message": "hello"}))
            .unwrap();
    let response = app
        .clone()
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

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_chat_injects_kg_memory_into_system_prompt() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Hello!", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;

    // Seed condensed memory in the knowledge graph
    state
        .knowledge_graph
        .set_condensed_memory("User enjoys hiking and sourdough bread.")
        .await
        .unwrap();

    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
    let response = app
        .clone()
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

    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 1, "expected one LLM chat call");
    let messages = &calls[0];
    assert!(!messages.is_empty(), "expected at least one message");
    assert_eq!(messages[0].role, "system");
    let content = &messages[0].content;
    // Issue #138: core-facts framing is third person with operating
    // directives appended; legacy wording is gone.
    assert_current_now_stamp(content);
    assert_eq!(
        content.matches("Now: ").count(),
        1,
        "system prompt must carry exactly one Now stamp: {content}"
    );
    assert!(
        content.contains("Core facts about the user"),
        "system prompt should contain the core-facts header"
    );
    assert!(
        content.contains("User enjoys hiking and sourdough bread."),
        "system prompt should contain the seeded KG memory"
    );
    assert!(
        content.contains("retrieve_context"),
        "system prompt should contain the retrieve_context directive"
    );
    assert!(
        !content.contains("Key facts I know about you:"),
        "system prompt must not contain legacy 'Key facts I know about you:'"
    );
    assert!(
        !content.contains("kg_query"),
        "system prompt must not surface internal kg_query tool"
    );
}

#[tokio::test]
async fn test_chat_refreshes_now_stamp_for_existing_session() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("first", Usage::default())
            .push_chat("second", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    state
        .knowledge_graph
        .set_condensed_memory("User enjoys hiking and sourdough bread.")
        .await
        .unwrap();
    let app = mimir_server::build_app(state.clone());

    let first = serde_json::to_string(&serde_json::json!({"message": "one"})).unwrap();
    let response = app
        .clone()
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/chat")
                .header("Content-Type", "application/json")
                .body(Body::from(first))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 1, "expected one LLM chat call: {calls:?}");
    let first_stamp = calls[0]
        .iter()
        .find(|message| message.role == "system")
        .unwrap_or_else(|| panic!("new sessions have a system prompt: {calls:?}"))
        .content
        .lines()
        .find_map(|line| line.strip_prefix("Now: "))
        .unwrap_or_else(|| panic!("new sessions carry a Now stamp: {:?}", calls[0]))
        .to_string();

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    let sessions = state.context_manager.list_sessions().await.unwrap();
    let second = serde_json::to_string(&serde_json::json!({
        "session_id": sessions[0].id,
        "message": "two"
    }))
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/chat")
                .header("Content-Type", "application/json")
                .body(Body::from(second))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let calls = mock.chat_calls();
    let second_stamp = calls[1][0]
        .content
        .lines()
        .find_map(|line| line.strip_prefix("Now: "))
        .expect("existing sessions still carry a Now stamp");
    assert_ne!(
        first_stamp, second_stamp,
        "existing-session system prompts must refresh the Now stamp"
    );
    assert_eq!(
        calls[1][0].content.matches("Now: ").count(),
        1,
        "refreshing the stamp must not duplicate the temporal anchor"
    );
    assert!(
        mock.chat_calls()[1][0]
            .content
            .contains("User enjoys hiking"),
        "refreshing the stamp must preserve the frozen memory block"
    );
}
#[tokio::test]
async fn test_chat_stream_injects_kg_memory_into_system_prompt() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("Hello!".to_string())),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;

    // Seed condensed memory in the knowledge graph
    state
        .knowledge_graph
        .set_condensed_memory("User enjoys hiking and sourdough bread.")
        .await
        .unwrap();

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

    // Consume the SSE body to ensure the spawned task runs.
    let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    let calls = mock.stream_calls();
    assert_eq!(calls.len(), 1, "expected one LLM stream call");
    let messages = &calls[0];
    assert!(!messages.is_empty(), "expected at least one message");
    assert_eq!(messages[0].role, "system");
    let content = &messages[0].content;
    // Issue #138: core-facts framing is third person with operating
    // directives appended; legacy wording is gone.
    assert_current_now_stamp(content);
    assert_eq!(
        content.matches("Now: ").count(),
        1,
        "stream prompt must carry exactly one Now stamp: {content}"
    );
    assert!(
        content.contains("Core facts about the user"),
        "system prompt should contain the core-facts header"
    );
    assert!(
        content.contains("User enjoys hiking and sourdough bread."),
        "system prompt should contain the seeded KG memory"
    );
    assert!(
        content.contains("retrieve_context"),
        "system prompt should contain the retrieve_context directive"
    );
    assert!(
        !content.contains("Key facts I know about you:"),
        "system prompt must not contain legacy 'Key facts I know about you:'"
    );
    assert!(
        !content.contains("kg_query"),
        "system prompt must not surface internal kg_query tool"
    );
}
