mod common;
use common::*;

#[tokio::test]
async fn test_chat_forwards_tools_to_llm() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Hello!", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
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
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "What time is it?"})).unwrap();
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
            // First stream: tool call + usage
            .push_stream(vec![
                Ok(StreamItem::ToolCalls(vec![tool_call_delta])),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            // Second stream (agentic loop): final text + usage
            .push_stream(vec![
                Ok(StreamItem::Text("The current time is now.".to_string())),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({"message": "What time is it?"})).unwrap();
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
    let text = String::from_utf8_lossy(&bytes);
    // The follow-up text should be streamed as a data event.
    assert!(
        text.contains("The current time is now."),
        "expected follow-up text in SSE stream, got: {}",
        text
    );

    // The tool_call SSE event should be present.
    assert!(
        text.contains("tool_call"),
        "expected tool_call event in SSE stream, got: {}",
        text
    );

    // The agentic loop should have made two stream calls.
    let calls = mock.stream_calls();
    assert_eq!(
        calls.len(),
        2,
        "expected two LLM stream calls (initial + agentic loop)"
    );
}
