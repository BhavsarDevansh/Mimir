//! Integration tests for the OpenAI-compatible provider surface (issue #388):
//! `GET /v1/models` and `POST /v1/chat/completions` (blocking + streaming).

mod common;
use common::*;

use mimir_api_types::{OpenAiChatResponse, OpenAiErrorBody, OpenAiModelList, OpenAiStreamChunk};

fn chat_body(model: &str, messages: serde_json::Value, extra: serde_json::Value) -> String {
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
    });
    if let serde_json::Value::Object(map) = &mut body {
        if let serde_json::Value::Object(extra) = extra {
            for (key, value) in extra {
                map.insert(key, value);
            }
        }
    }
    serde_json::to_string(&body).unwrap()
}

async fn post_v1_chat(
    app: &axum::Router,
    body: &str,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, headers, value)
}

#[tokio::test]
async fn test_v1_models_lists_presets() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: OpenAiModelList = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.object, "list");
    let transparent = list
        .data
        .iter()
        .find(|model| model.id == "transparent")
        .expect("transparent preset listed");
    assert_eq!(transparent.object, "model");
    assert_eq!(transparent.created, 0);
    assert_eq!(transparent.owned_by, "mimir");
    assert!(
        transparent.description.is_some(),
        "built-in presets carry descriptions"
    );
    assert!(
        list.data.iter().any(|model| model.id == "concise"),
        "all built-in presets listed"
    );
}

#[tokio::test]
async fn test_v1_models_requires_auth() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_v1_chat_blocking_basic_shape() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Hello!", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _headers, value) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::OK);
    let response: OpenAiChatResponse = serde_json::from_value(value).unwrap();
    assert!(response.id.starts_with("chatcmpl-"), "id: {}", response.id);
    assert_eq!(response.object, "chat.completion");
    assert_eq!(response.model, "gpt-4o");
    assert_eq!(response.choices.len(), 1);
    assert_eq!(response.choices[0].index, 0);
    assert_eq!(response.choices[0].message.role, "assistant");
    assert_eq!(
        response.choices[0].message.content.as_deref(),
        Some("Hello!")
    );
    assert_eq!(response.choices[0].finish_reason, "stop");
    assert!(response.choices[0].message.tool_calls.is_empty());
}

#[tokio::test]
async fn test_v1_chat_same_user_resumes_one_session() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("first", Usage::default())
            .push_chat("second", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let first = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "one"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, _) = post_v1_chat(&app, &first).await;
    assert_eq!(status, StatusCode::OK);

    let second = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "two"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, _) = post_v1_chat(&app, &second).await;
    assert_eq!(status, StatusCode::OK);

    // One session, and the second LLM call saw the first turn's history.
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1, "same user key must resume one session");
    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 2);
    let second_conversation = &calls[1];
    assert!(
        second_conversation
            .iter()
            .any(|m| m.role == "user" && m.content == "one"),
        "second turn must include the first turn's history: {second_conversation:?}"
    );
    assert!(
        second_conversation
            .iter()
            .any(|m| m.role == "user" && m.content == "two"),
        "second turn must include its own user message"
    );
}

#[tokio::test]
async fn test_v1_chat_without_user_is_incognito() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Hello!", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({}),
    );
    let (status, _, _) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::OK);

    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert!(
        sessions.is_empty(),
        "requests without `user` must not persist a session"
    );
}

#[tokio::test]
async fn test_v1_chat_preset_model_selects_personality() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("ok", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "concise",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, _) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::OK);

    let calls = mock.chat_calls();
    let system = calls[0]
        .iter()
        .find(|m| m.role == "system")
        .expect("system prompt present");
    assert!(
        system
            .content
            .contains("Use minimal words and maximum information density"),
        "concise preset prompt must be used: {}",
        system.content
    );
}

#[tokio::test]
async fn test_v1_chat_client_tool_roundtrip() {
    let client_tool_call = ToolCall {
        index: 0,
        id: "call_1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "get_stock_price".to_string(),
            arguments: "{\"location\":\"London\"}".to_string(),
        },
    };
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_calls: Some(vec![client_tool_call.clone()]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
            .push_chat("The price is 100.", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let tools = serde_json::json!([{
        "type": "function",
        "function": {
            "name": "get_stock_price",
            "description": "Get the stock price",
            "parameters": {"type": "object", "properties": {"symbol": {"type": "string"}}}
        }
    }]);
    let first = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "stock price?"}]),
        serde_json::json!({"user": "phone", "tools": tools}),
    );
    let (status, _, value) = post_v1_chat(&app, &first).await;
    assert_eq!(status, StatusCode::OK);
    let response: OpenAiChatResponse = serde_json::from_value(value).unwrap();
    assert_eq!(response.choices[0].finish_reason, "tool_calls");
    assert_eq!(response.choices[0].message.content, None);
    assert_eq!(response.choices[0].message.tool_calls.len(), 1);
    assert_eq!(
        response.choices[0].message.tool_calls[0].function.name,
        "get_stock_price"
    );
    assert_eq!(response.choices[0].message.tool_calls[0].id, "call_1");

    // The client executes the tool and sends the result back; the turn
    // continues and completes with the final answer.
    let follow_up = chat_body(
        "gpt-4o",
        serde_json::json!([
            {"role": "user", "content": "stock price?"},
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "get_stock_price", "arguments": "{\"symbol\":\"AAPL\"}"}
                }]
            },
            {"role": "tool", "tool_call_id": "call_1", "content": "100"}
        ]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, value) = post_v1_chat(&app, &follow_up).await;
    assert_eq!(status, StatusCode::OK);
    let response: OpenAiChatResponse = serde_json::from_value(value).unwrap();
    assert_eq!(response.choices[0].finish_reason, "stop");
    assert_eq!(
        response.choices[0].message.content.as_deref(),
        Some("The price is 100.")
    );

    // The second LLM call saw the assistant tool-call message and the tool
    // result, and the session persisted both.
    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 2);
    let conversation = &calls[1];
    assert!(
        conversation.iter().any(|m| m.role == "assistant"
            && m.tool_calls
                .as_ref()
                .is_some_and(|calls| calls[0].id == "call_1")),
        "follow-up conversation must include the assistant tool-call message: {conversation:?}"
    );
    assert!(
        conversation.iter().any(|m| m.role == "tool"
            && m.tool_call_id.as_deref() == Some("call_1")
            && m.content == "100"),
        "follow-up conversation must include the tool result: {conversation:?}"
    );
}

#[tokio::test]
async fn test_v1_chat_server_tool_wins_name_collision() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("ok", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    // The client sends a tool whose name collides with the server-side
    // `echo` tool; the server definition must win and the client's
    // definition must be silently dropped.
    let tools = serde_json::json!([{
        "type": "function",
        "function": {
            "name": "echo",
            "description": "CLIENT VERSION MUST NOT REACH THE LLM",
            "parameters": {"type": "object"}
        }
    }]);
    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({"user": "phone", "tools": tools}),
    );
    let (status, _, _) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::OK);

    let tool_lists = mock.chat_tools();
    let merged = tool_lists[0].as_ref().expect("tools sent to the LLM");
    let echo_defs: Vec<&serde_json::Value> = merged
        .iter()
        .filter(|tool| tool["function"]["name"] == "echo")
        .collect();
    assert_eq!(echo_defs.len(), 1, "server tool must win the collision");
    assert!(
        !echo_defs[0].to_string().contains("CLIENT VERSION"),
        "client definition must be dropped: {}",
        echo_defs[0]
    );
}

#[tokio::test]
async fn test_v1_chat_server_tool_executes_internally() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_calls: Some(vec![ToolCall {
                        index: 0,
                        id: "call_echo".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "echo".to_string(),
                            arguments: "{\"message\":\"ping\"}".to_string(),
                        },
                    }]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
            .push_chat("pong", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "echo ping"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, value) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::OK);
    let response: OpenAiChatResponse = serde_json::from_value(value).unwrap();
    assert_eq!(response.choices[0].finish_reason, "stop");
    assert_eq!(response.choices[0].message.content.as_deref(), Some("pong"));

    // The server executed `echo` internally and fed the result back to the
    // LLM; the client never saw the tool call.
    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[1]
            .iter()
            .any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call_echo")),
        "second LLM call must include the echo tool result: {:?}",
        calls[1]
    );
}

#[tokio::test]
async fn test_v1_chat_stream_framing() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("Hello".to_string())),
                Ok(StreamItem::Text(" world".to_string())),
                Ok(StreamItem::Usage(Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                })),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({"user": "phone", "stream": true}),
    );
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/v1/chat/completions")
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

    let chunks: Vec<OpenAiStreamChunk> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();

    assert!(
        text.trim_end().ends_with("data: [DONE]"),
        "stream must end with [DONE]: {text}"
    );
    assert_eq!(chunks[0].object, "chat.completion.chunk");
    assert_eq!(
        chunks[0].choices[0].delta.role.as_deref(),
        Some("assistant"),
        "first chunk must carry the assistant role"
    );
    let content: String = chunks
        .iter()
        .filter_map(|chunk| chunk.choices.first())
        .filter_map(|choice| choice.delta.content.clone())
        .collect();
    assert_eq!(content, "Hello world");
    let last = chunks.last().unwrap();
    assert_eq!(
        last.choices[0].finish_reason.as_deref(),
        Some("stop"),
        "final chunk must carry finish_reason stop"
    );
    assert!(
        chunks.iter().all(|chunk| chunk.usage.is_none()),
        "usage chunk must not be sent without include_usage"
    );
}

#[tokio::test]
async fn test_v1_chat_stream_include_usage() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("hi".to_string())),
                Ok(StreamItem::Usage(Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                })),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({
            "user": "phone",
            "stream": true,
            "stream_options": {"include_usage": true}
        }),
    );
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/v1/chat/completions")
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

    let chunks: Vec<OpenAiStreamChunk> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();

    let usage_chunk = chunks
        .iter()
        .find(|chunk| chunk.usage.is_some())
        .expect("usage chunk present with include_usage");
    assert!(usage_chunk.choices.is_empty());
    assert_eq!(usage_chunk.usage.as_ref().unwrap().total_tokens, 5);
}

#[tokio::test]
async fn test_v1_chat_stream_client_tool_deltas() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::ToolCalls(vec![ToolCall {
                    index: 0,
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_stock_price".to_string(),
                        arguments: "{\"location\":".to_string(),
                    },
                }])),
                Ok(StreamItem::ToolCalls(vec![ToolCall {
                    index: 0,
                    id: String::new(),
                    call_type: String::new(),
                    function: FunctionCall {
                        name: String::new(),
                        arguments: "\"London\"}".to_string(),
                    },
                }])),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let tools = serde_json::json!([{
        "type": "function",
        "function": {
            "name": "get_stock_price",
            "description": "Get the stock price",
            "parameters": {"type": "object"}
        }
    }]);
    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "stock price?"}]),
        serde_json::json!({"user": "phone", "stream": true, "tools": tools}),
    );
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/v1/chat/completions")
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

    let chunks: Vec<OpenAiStreamChunk> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();

    let tool_deltas: Vec<_> = chunks
        .iter()
        .filter_map(|chunk| chunk.choices.first())
        .filter_map(|choice| choice.delta.tool_calls.first())
        .collect();
    assert_eq!(tool_deltas.len(), 2, "both tool-call deltas streamed");
    assert_eq!(tool_deltas[0].id.as_deref(), Some("call_1"));
    assert_eq!(
        tool_deltas[0].function.as_ref().unwrap().name,
        "get_stock_price"
    );
    let last = chunks.last().unwrap();
    assert_eq!(
        last.choices[0].finish_reason.as_deref(),
        Some("tool_calls"),
        "stream must end with finish_reason tool_calls"
    );
    assert!(text.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn test_v1_chat_queue_full_returns_503_openai_shape() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(LlmError::QueueFull)
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, headers, value) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "5"
    );
    let error: OpenAiErrorBody = serde_json::from_value(value).unwrap();
    assert_eq!(error.error.error_type, "server_error");
    assert_eq!(error.error.code.as_deref(), Some("queue_full"));
}

#[tokio::test]
async fn test_v1_chat_invalid_json_returns_400_openai_shape() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let (status, _, value) = post_v1_chat(&app, "{not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: OpenAiErrorBody = serde_json::from_value(value).unwrap();
    assert_eq!(error.error.error_type, "invalid_request_error");
}

#[tokio::test]
async fn test_v1_chat_missing_user_message_returns_400() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "assistant", "content": "hi"}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, value) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: OpenAiErrorBody = serde_json::from_value(value).unwrap();
    assert_eq!(error.error.param.as_deref(), Some("messages"));
}

#[tokio::test]
async fn test_v1_chat_empty_user_message_returns_400() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "   "}]),
        serde_json::json!({"user": "phone"}),
    );
    let (status, _, value) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: OpenAiErrorBody = serde_json::from_value(value).unwrap();
    assert_eq!(error.error.error_type, "invalid_request_error");
    assert_eq!(error.error.param.as_deref(), Some("messages"));
}

#[tokio::test]
async fn test_v1_chat_requires_auth() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "hi"}]),
        serde_json::json!({}),
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
