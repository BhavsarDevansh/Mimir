mod common;
use common::*;

use async_trait::async_trait;
use mimir_core::llm::LlmStream;

/// Test-only backend that resolves model overrides to a distinct inner
/// backend, mirroring `LlmClient::with_model_override` so chat tests can
/// prove the request path uses the request-resolved LLM rather than the
/// application-startup LLM (issue #441).
#[derive(Debug)]
struct ModelOverrideBackend {
    default: Arc<dyn LlmBackend>,
    overrides: DashMap<String, Arc<dyn LlmBackend>>,
}

impl ModelOverrideBackend {
    fn new(
        default: Arc<dyn LlmBackend>,
        overrides: impl IntoIterator<Item = (String, Arc<dyn LlmBackend>)>,
    ) -> Self {
        Self {
            default,
            overrides: DashMap::from_iter(overrides),
        }
    }
}

#[async_trait]
impl LlmBackend for ModelOverrideBackend {
    async fn chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        self.default.chat_message(messages, tools).await
    }

    async fn chat_stream_with_usage(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LlmStream, LlmError> {
        self.default.chat_stream_with_usage(messages, tools).await
    }

    async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError> {
        self.default.fetch_model_context_window().await
    }

    fn with_model_override(&self, model: String) -> Option<Arc<dyn LlmBackend>> {
        self.overrides
            .get(&model)
            .map(|backend| Arc::clone(&*backend))
    }
}

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

#[tokio::test]
async fn test_chat_executes_retrieve_context_through_registry() {
    // The main chat call asks the model to run `retrieve_context`; the
    // retrieval agent then runs two internal rounds (kg_query, finish) on
    // the same request-resolved LLM before the main chat produces its final
    // answer (issue #441).
    let retrieve_call = ToolCall {
        index: 0,
        id: "call_retrieve".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "retrieve_context".to_string(),
            arguments: serde_json::json!({"task": "Find Alice"}).to_string(),
        },
    };
    let query_call = ToolCall {
        index: 0,
        id: "call_query".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "kg_query".to_string(),
            arguments: serde_json::json!({"entity_name": "Alice"}).to_string(),
        },
    };
    let finish_call = ToolCall {
        index: 0,
        id: "call_finish".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "finish_retrieval".to_string(),
            arguments: "{}".to_string(),
        },
    };
    // The request resolves a *distinct* LLM backend through the model
    // override, so the retrieval agent can only succeed if the registry
    // factory rebuilds `retrieve_context` with the request-resolved LLM
    // (`ctx.llm`) rather than the startup LLM captured at registration
    // (issue #441).
    let request_mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: "".to_string(),
                    tool_calls: Some(vec![retrieve_call]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: "".to_string(),
                    tool_calls: Some(vec![query_call]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: "".to_string(),
                    tool_calls: Some(vec![finish_call]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
            .push_chat("Found Alice's preferences.", Usage::default())
            .build(),
    );
    // The startup backend has no queued responses: any request-path call
    // reaching it would fail the empty-calls assertion below.
    let startup_mock = Arc::new(MockLlmClient::builder().build());
    let switchboard = Arc::new(ModelOverrideBackend::new(
        startup_mock.clone(),
        [(
            "retrieval-test-model".to_string(),
            request_mock.clone() as Arc<dyn LlmBackend>,
        )],
    ));
    let (state, _temp) = test_state(switchboard.clone()).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "What do I know about Alice?",
        "model": "retrieval-test-model",
    }))
    .unwrap();
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
    assert_eq!(chat.response, "Found Alice's preferences.");

    // Four LLM calls: main chat (retrieve_context) + two retrieval-agent
    // rounds + main chat follow-up, all on the request-resolved backend.
    // The retrieval agent's first call carries the research system prompt,
    // proving the registry factory passed the request-resolved LLM through.
    let calls = request_mock.chat_calls();
    assert_eq!(
        calls.len(),
        4,
        "expected main chat + retrieval agent + follow-up calls on the request-resolved backend"
    );
    assert!(
        calls[1][0].content.contains("research subsystem"),
        "retrieval agent should run on the request-resolved LLM"
    );
    assert!(
        startup_mock.chat_calls().is_empty(),
        "a factory capturing the startup LLM would route request-path calls to the startup backend"
    );
}
