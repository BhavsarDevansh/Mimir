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
    let calls = mock.chat_calls();
    assert!(!calls.is_empty(), "expected one LLM chat call");
    let system = calls[0]
        .iter()
        .find(|m| m.role == "system")
        .expect("system prompt present");
    assert_current_now_stamp(&system.content);
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
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

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
    let first_stamp = calls[0]
        .iter()
        .find(|m| m.role == "system")
        .expect("first OpenAI turn has a system prompt")
        .content
        .lines()
        .find_map(|line| line.strip_prefix("Now: "))
        .expect("first OpenAI turn carries a Now stamp")
        .to_string();
    let second_system = calls[1]
        .iter()
        .find(|m| m.role == "system")
        .expect("second OpenAI turn has a system prompt");
    assert_current_now_stamp(&second_system.content);
    assert_ne!(
        first_stamp,
        second_system
            .content
            .lines()
            .find_map(|line| line.strip_prefix("Now: "))
            .expect("second OpenAI turn carries a Now stamp"),
        "existing OpenAI sessions must refresh the Now stamp"
    );
    assert_eq!(
        second_system.content.matches("Now: ").count(),
        1,
        "refreshing the stamp must not duplicate it: {}",
        second_system.content
    );
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
async fn test_v1_chat_without_user_persists_default_session_and_learns() {
    // Issue #473: a request without `user` is never incognito — it keys the
    // fixed default session, persists the turn, and fires the learning hook.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Noted.", Usage::default())
            .push_chat_message(extraction_message(), Usage::default())
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "My favourite colour is blue."}]),
        serde_json::json!({}),
    );
    let (status, _, _) = post_v1_chat(&app, &body).await;
    assert_eq!(status, StatusCode::OK);

    let session_id = state
        .context_manager
        .find_session_by_user_key("default")
        .await
        .unwrap()
        .expect("requests without `user` must resolve the default session");
    let messages = state
        .context_manager
        .export_conversation(session_id)
        .await
        .unwrap()
        .messages;
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "My favourite colour is blue.");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[2].content, "Noted.");

    assert!(
        wait_for_prefers_blue(&state).await,
        "the remember.chat hook must fire for unkeyed requests and persist the fact"
    );
}

#[tokio::test]
async fn test_v1_chat_without_user_resumes_default_session() {
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
        serde_json::json!({}),
    );
    let (status, _, _) = post_v1_chat(&app, &first).await;
    assert_eq!(status, StatusCode::OK);

    // A blank `user` is treated as absent and resumes the same default
    // session rather than keying a session on "".
    let second = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "two"}]),
        serde_json::json!({"user": "   "}),
    );
    let (status, _, _) = post_v1_chat(&app, &second).await;
    assert_eq!(status, StatusCode::OK);

    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "blank `user` must resume the default session"
    );
    assert_eq!(
        state
            .context_manager
            .find_session_by_user_key("default")
            .await
            .unwrap(),
        Some(sessions[0].id),
        "the default session must be keyed `default`"
    );
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
async fn test_v1_chat_stream_without_user_persists_and_learns() {
    // The streaming path has the same contract as blocking: no `user` means
    // the default persistent session, not incognito (issue #473).
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("Noted.".to_string())),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .push_chat_message(extraction_message(), Usage::default())
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "My favourite colour is blue."}]),
        serde_json::json!({"stream": true}),
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
    assert!(
        text.contains("Noted."),
        "stream must carry the response: {text}"
    );
    assert!(text.trim_end().ends_with("data: [DONE]"));

    let session_id = state
        .context_manager
        .find_session_by_user_key("default")
        .await
        .unwrap()
        .expect("unkeyed stream must resolve the default session");
    let messages = state
        .context_manager
        .export_conversation(session_id)
        .await
        .unwrap()
        .messages;
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[2].content, "Noted.");

    assert!(
        wait_for_prefers_blue(&state).await,
        "unkeyed stream turns must fire the learning hook and persist the fact"
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
            arguments: "{\"symbol\":\"AAPL\"}".to_string(),
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
    assert_eq!(
        response.choices[0].message.tool_calls[0].function.arguments, "{\"symbol\":\"AAPL\"}",
        "the tool-call arguments must match the declared schema"
    );

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
        .flat_map(|choice| choice.delta.tool_calls.iter())
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
async fn test_v1_chat_stream_multiple_client_tools_streamed_in_index_order() {
    let mock = Arc::new(
        MockLlmClient::builder()
            // Delivered out of order: index 1 arrives before index 0, and
            // each call arrives as two deltas (header + arguments).
            .push_stream(vec![
                Ok(StreamItem::ToolCalls(vec![ToolCall {
                    index: 1,
                    id: "call_2".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_stock_price".to_string(),
                        arguments: "{\"symbol\":".to_string(),
                    },
                }])),
                Ok(StreamItem::ToolCalls(vec![ToolCall {
                    index: 0,
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_stock_price".to_string(),
                        arguments: "{\"symbol\":".to_string(),
                    },
                }])),
                Ok(StreamItem::ToolCalls(vec![ToolCall {
                    index: 1,
                    id: String::new(),
                    call_type: String::new(),
                    function: FunctionCall {
                        name: String::new(),
                        arguments: "\"MSFT\"}".to_string(),
                    },
                }])),
                Ok(StreamItem::ToolCalls(vec![ToolCall {
                    index: 0,
                    id: String::new(),
                    call_type: String::new(),
                    function: FunctionCall {
                        name: String::new(),
                        arguments: "\"AAPL\"}".to_string(),
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
        serde_json::json!([{"role": "user", "content": "prices?"}]),
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
        .flat_map(|choice| choice.delta.tool_calls.iter())
        .collect();
    let indices: Vec<u32> = tool_deltas.iter().map(|delta| delta.index).collect();
    assert_eq!(
        indices.len(),
        4,
        "all buffered deltas must be emitted; stream text: {text:?}"
    );
    assert!(
        indices.windows(2).all(|pair| pair[0] <= pair[1]),
        "emitted tool-call indices must be non-decreasing: {indices:?}"
    );
    assert_eq!(indices[0], 0);
    assert_eq!(indices[2], 1);
    assert_eq!(tool_deltas[0].id.as_deref(), Some("call_1"));
    assert_eq!(tool_deltas[2].id.as_deref(), Some("call_2"));
    let last = chunks.last().unwrap();
    assert_eq!(
        last.choices[0].finish_reason.as_deref(),
        Some("tool_calls"),
        "stream must end with finish_reason tool_calls"
    );
    assert!(text.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn test_v1_chat_stream_error_sends_error_event_and_done() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("partial".to_string())),
                Err(LlmError::RetryExhausted {
                    attempts: 3,
                    last_error: Box::new(LlmError::Api {
                        status: 503,
                        body: "overloaded".to_string(),
                    }),
                }),
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

    assert!(
        text.contains("event: error"),
        "a failed stream must emit an error event: {text:?}"
    );
    assert!(
        text.contains("API error 503: overloaded"),
        "the error event must surface the LLM failure detail: {text:?}"
    );
    assert!(
        text.trim_end().ends_with("data: [DONE]"),
        "a failed stream must terminate with [DONE]: {text:?}"
    );

    // The failed turn must not leave the persisted user message behind.
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    let msgs = state
        .context_manager
        .export_conversation(sessions[0].id)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        msgs.len(),
        1,
        "failed stream must roll back the request's messages: {msgs:?}"
    );
    assert_eq!(msgs[0].role, "system");
}

#[tokio::test]
async fn test_v1_chat_stream_server_tool_deltas_not_streamed_to_client() {
    let mock = Arc::new(
        MockLlmClient::builder()
            // Round 1: the model calls the server-side `echo` tool. The
            // deltas must be executed internally and never reach the client.
            .push_stream(vec![Ok(StreamItem::ToolCalls(vec![ToolCall {
                index: 0,
                id: "call_echo".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "echo".to_string(),
                    arguments: "{\"message\":\"ping\"}".to_string(),
                },
            }]))])
            .push_stream(vec![
                Ok(StreamItem::Text("pong".to_string())),
                Ok(StreamItem::Usage(Usage {
                    prompt_tokens: 5,
                    completion_tokens: 3,
                    total_tokens: 8,
                })),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "echo ping"}]),
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
        !text.contains("\"tool_calls\""),
        "server-side tool deltas must never reach the client: {text:?}"
    );
    let content: String = chunks
        .iter()
        .filter_map(|chunk| chunk.choices.first())
        .filter_map(|choice| choice.delta.content.clone())
        .collect();
    assert_eq!(content, "pong");
    let last = chunks.last().unwrap();
    assert_eq!(
        last.choices[0].finish_reason.as_deref(),
        Some("stop"),
        "a server-tool round must end with the final answer, not tool_calls"
    );
    assert!(text.trim_end().ends_with("data: [DONE]"));

    // The server executed the tool internally and persisted the round.
    let sessions = state.context_manager.list_sessions().await.unwrap();
    let msgs = state
        .context_manager
        .export_conversation(sessions[0].id)
        .await
        .unwrap()
        .messages;
    assert_eq!(msgs.len(), 5, "full round persisted: {msgs:?}");
    assert_eq!(msgs[2].role, "assistant");
    assert!(msgs[2].tool_calls.is_some(), "tool-call message persisted");
    assert_eq!(msgs[3].role, "tool");
    assert_eq!(msgs[4].content, "pong");
}

#[tokio::test]
async fn test_v1_chat_invalid_client_tool_returns_400_with_param() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    for tools in [
        serde_json::json!([{"type": "not_a_function", "function": {"name": "x"}}]),
        serde_json::json!([{"type": "function", "function": {"name": "x", "parameters": "nope"}}]),
        serde_json::json!([{"type": "function", "function": {"name": ""}}]),
        serde_json::json!([{"function": {"name": "x"}}]),
    ] {
        let body = chat_body(
            "gpt-4o",
            serde_json::json!([{"role": "user", "content": "hi"}]),
            serde_json::json!({"user": "phone", "tools": tools}),
        );
        let (status, _, value) = post_v1_chat(&app, &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "tools: {tools}");
        let error: OpenAiErrorBody = serde_json::from_value(value).unwrap();
        assert_eq!(error.error.error_type, "invalid_request_error");
        assert_eq!(error.error.param.as_deref(), Some("tools"));
    }

    // Validation runs before session creation and persistence, so a rejected
    // request must not leave a session or an orphaned user message behind.
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert!(sessions.is_empty(), "no session for rejected tools");
}

#[tokio::test]
async fn test_v1_chat_blocking_usage_accumulates_across_tool_rounds() {
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
                Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                },
            )
            .push_chat(
                "pong",
                Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                },
            )
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
    assert_eq!(response.usage.prompt_tokens, 13, "usage must accumulate");
    assert_eq!(response.usage.completion_tokens, 7);
    assert_eq!(response.usage.total_tokens, 20);
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

    // The failed turn must not leave the persisted user message behind: the
    // session keeps only its system prompt (PR #466 review).
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    let msgs = state
        .context_manager
        .export_conversation(sessions[0].id)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        msgs.len(),
        1,
        "failed turn must roll back the request's messages: {msgs:?}"
    );
    assert_eq!(msgs[0].role, "system");
}

#[tokio::test]
async fn test_v1_chat_stream_queue_full_returns_503_before_sse() {
    // PR #477 review: queue admission must happen before the SSE response
    // starts, so a full user queue returns the documented 503 + Retry-After
    // instead of an SSE error event after `200 OK`.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream_error(LlmError::QueueFull)
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
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        "5"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        !text.contains("event: error") && !text.contains("data: [DONE]"),
        "queue-full admission must not start an SSE stream: {text:?}"
    );
    let error: OpenAiErrorBody = serde_json::from_str(&text).unwrap();
    assert_eq!(error.error.error_type, "server_error");
    assert_eq!(error.error.code.as_deref(), Some("queue_full"));

    // The rejected turn must not leave the persisted user message behind.
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    let msgs = state
        .context_manager
        .export_conversation(sessions[0].id)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        msgs.len(),
        1,
        "failed stream must roll back the request's messages: {msgs:?}"
    );
    assert_eq!(msgs[0].role, "system");
}

#[tokio::test]
async fn test_v1_chat_stream_first_attempt_failure_returns_detailed_500() {
    // PR #490 review: a provider outage on the first stream attempt must
    // surface the bounded failure detail in the HTTP error body instead of a
    // generic "internal server error", while queue-full keeps its 503.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream_error(LlmError::RetryExhausted {
                attempts: 4,
                last_error: Box::new(LlmError::Api {
                    status: 503,
                    body: "model temporarily overloaded".to_string(),
                }),
            })
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
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        !text.contains("event: error") && !text.contains("data: [DONE]"),
        "a pre-SSE failure must not start an SSE stream: {text:?}"
    );
    let error: OpenAiErrorBody = serde_json::from_str(&text).unwrap();
    assert_eq!(error.error.error_type, "server_error");
    assert!(
        error.error.message.contains("model temporarily overloaded"),
        "the first-attempt failure must carry the bounded detail: {text:?}"
    );

    // The rejected turn must not leave the persisted user message behind.
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    let msgs = state
        .context_manager
        .export_conversation(sessions[0].id)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        msgs.len(),
        1,
        "failed stream must roll back the request's messages: {msgs:?}"
    );
    assert_eq!(msgs[0].role, "system");
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

#[tokio::test]
async fn test_v1_chat_stream_client_disconnect_rolls_back_persisted_turn() {
    // PR #480 review: if the SSE receiver closes before the turn completes,
    // the stream task must roll back the persisted user/tool messages
    // instead of leaving an orphaned final turn in the session.
    let mock = Arc::new(
        MockLlmClient::builder()
            // Round 1: the model calls the server-side `echo` tool, which
            // persists an assistant tool-call message and a tool result
            // before round 2 starts.
            .push_stream(vec![Ok(StreamItem::ToolCalls(vec![ToolCall {
                index: 0,
                id: "call_echo".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "echo".to_string(),
                    arguments: "{\"message\":\"ping\"}".to_string(),
                },
            }]))])
            // Round 2: a long text stream, so the stream task is still
            // sending chunks when the client disconnects.
            .push_stream(
                (0..30)
                    .map(|_| Ok(StreamItem::Text("ping".to_string())))
                    .collect(),
            )
            .build(),
    );
    let (state, _temp) = test_state(mock.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = chat_body(
        "gpt-4o",
        serde_json::json!([{"role": "user", "content": "echo ping"}]),
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

    // Simulate a client that disconnects without reading the stream: the
    // stream task's next `send_chunk` fails and it must roll the persisted
    // turn back.
    drop(response);

    // Give the stream task time to observe the closed receiver and roll
    // back before asserting on the session.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1, "session created by the request");
    let msgs = state
        .context_manager
        .export_conversation(sessions[0].id)
        .await
        .unwrap()
        .messages;
    assert!(
        msgs.iter().all(|m| m.role == "system"),
        "no user, assistant, or tool messages may remain after a cancelled stream: {msgs:?}"
    );
}
