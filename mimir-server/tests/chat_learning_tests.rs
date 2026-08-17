mod common;
use common::*;

#[tokio::test]
async fn test_chat_extracts_facts_after_response() {
    // Inline learning (issue #137): the conversational LLM calls the
    // `remember` tool while composing its reply, so facts are persisted
    // during the chat turn itself — no background Librarian required.
    let remember_output = mimir_knowledge::extract::RememberOutput {
        facts: vec![mimir_knowledge::extract::ExtractedFact {
            classification: mimir_knowledge::extract::Classification::Explicit,
            subject: "Devansh".to_string(),
            subject_type: "Person".to_string(),
            relationship_type: "favourite_colour".to_string(),
            object: "blue".to_string(),
            object_is_entity: false,
            object_type: None,
            temporal: None,
            is_sensitive: false,
            correction_scope: None,
            categories: vec![],
            recurrence: None,
            requires_user_action: None,
            location: None,
        }],
    };
    let extraction_msg = Message {
        role: "assistant".to_string(),
        content: "".to_string(),
        tool_calls: Some(vec![ToolCall {
            index: 0,
            id: "call_remember".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: serde_json::to_string(&remember_output).unwrap(),
            },
        }]),
        tool_call_id: None,
    };

    // The LLM orchestrates learning inline: its first response calls the
    // `remember` tool to persist the fact, then it produces a final
    // acknowledgement. There is no separate background extraction pass.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(extraction_msg, Usage::default())
            .push_chat("Got it!", Usage::default())
            .build(),
    );

    let mut config = Config::default();
    config.identity.name = "Devansh".to_string();
    let (state, _temp) = test_state_with_config(mock, config).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "My favourite colour is blue."
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

    // Poll with timeout so the test is deterministic, not timing-dependent.
    let mut found = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let search = state
            .knowledge_graph
            .search_entities("Devansh", 1)
            .await
            .unwrap();
        if search.is_empty() {
            continue;
        }
        let entity = &search[0].entity;

        let facts = state
            .knowledge_graph
            .get_facts_by_subject(entity.id, 100)
            .await
            .unwrap();

        for f in &facts {
            let pred = state
                .knowledge_graph
                .relationship_type_name(f.relationship_type_id)
                .await;
            if pred.as_deref() == Some("favourite_colour")
                && f.object_literal.as_deref() == Some("blue")
            {
                found = true;
                break;
            }
        }
        if found {
            break;
        }
    }

    assert!(
        found,
        "expected favourite_colour=blue fact to be extracted within 2.5s"
    );
}
#[tokio::test]
async fn test_remember_tool_executes_and_writes_facts() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    // Call the remember tool directly through the registry.
    let args = serde_json::json!({
        "facts": [
            {
                "classification": "Explicit",
                "subject": "Alice",
                "subject_type": "Person",
                "relationship_type": "favourite_colour",
                "object": "red",
                "object_is_entity": false,
                "is_sensitive": false,
                "categories": []
            }
        ]
    });

    let output = state
        .tool_registry
        .execute("remember", args)
        .await
        .expect("remember tool should succeed");

    let text = output.to_llm_text();
    assert!(
        text.contains("inserted") || text.contains("matched"),
        "expected success text, got: {}",
        text
    );

    // Verify the fact exists.
    let search = state
        .knowledge_graph
        .search_entities("Alice", 1)
        .await
        .unwrap();
    assert!(!search.is_empty(), "expected entity 'Alice' to be created");
    let entity = &search[0].entity;

    let facts = state
        .knowledge_graph
        .get_facts_by_subject(entity.id, 100)
        .await
        .unwrap();

    let mut found = false;
    for f in &facts {
        let pred = state
            .knowledge_graph
            .relationship_type_name(f.relationship_type_id)
            .await;
        if pred.as_deref() == Some("favourite_colour") && f.object_literal.as_deref() == Some("red")
        {
            found = true;
            break;
        }
    }

    assert!(
        found,
        "expected favourite_colour=red fact to be written via remember tool"
    );
}
#[tokio::test]
async fn test_incognito_blocks_remember_tool_and_writes_no_facts() {
    let tool_call = ToolCall {
        index: 0,
        id: "call_remember".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "remember".to_string(),
            arguments: serde_json::json!({
                "facts": [{
                    "classification": "Explicit",
                    "subject": "Incognito Test User",
                    "subject_type": "Person",
                    "relationship_type": "based_in",
                    "object": "London",
                    "object_is_entity": false,
                    "is_sensitive": false,
                    "categories": []
                }]
            })
            .to_string(),
        },
    };
    let first = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![tool_call]),
        tool_call_id: None,
    };
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(first, Usage::default())
            .push_chat("Noted.", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let kg = Arc::clone(&state.knowledge_graph);
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "remember that I am based in London",
        "incognito": true,
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

    // No entity/fact should have been created during the incognito turn.
    let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
    assert!(
        found.is_empty(),
        "incognito turn must not persist entities, got: {found:?}"
    );
}
#[tokio::test]
async fn test_non_incognito_allows_remember_tool_and_persists_fact() {
    // Control: the same tool call persists a fact when not incognito,
    // proving the incognito guard is what prevents writes (issue #155).
    let tool_call = ToolCall {
        index: 0,
        id: "call_remember".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "remember".to_string(),
            arguments: serde_json::json!({
                "facts": [{
                    "classification": "Explicit",
                    "subject": "Incognito Test User",
                    "subject_type": "Person",
                    "relationship_type": "based_in",
                    "object": "London",
                    "object_is_entity": false,
                    "is_sensitive": false,
                    "categories": []
                }]
            })
            .to_string(),
        },
    };
    let first = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![tool_call]),
        tool_call_id: None,
    };
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(first, Usage::default())
            .push_chat("Noted.", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let kg = Arc::clone(&state.knowledge_graph);
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "remember that I am based in London",
        "incognito": false,
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

    let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
    assert!(
        !found.is_empty(),
        "non-incognito turn should persist the entity/fact"
    );
}
#[tokio::test]
async fn test_incognito_blocks_remember_tool_and_writes_no_facts_stream() {
    let tool_call = ToolCall {
        index: 0,
        id: "call_remember".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "remember".to_string(),
            arguments: serde_json::json!({
                "facts": [{
                    "classification": "Explicit",
                    "subject": "Incognito Test User",
                    "subject_type": "Person",
                    "relationship_type": "based_in",
                    "object": "London",
                    "object_is_entity": false,
                    "is_sensitive": false,
                    "categories": []
                }]
            })
            .to_string(),
        },
    };
    // The streaming endpoint reads from the mock's stream-response queue
    // (push_stream / StreamItem), not the blocking chat-message queue, so
    // queue a `remember` tool-call stream followed by the final reply.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::ToolCalls(vec![tool_call])),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .push_stream(vec![
                Ok(StreamItem::Text("Noted.".to_string())),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let kg = Arc::clone(&state.knowledge_graph);
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "remember that I am based in London",
        "incognito": true,
    }))
    .unwrap();
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

    // Drain the SSE response body to ensure stream processing completes.
    let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    // No entity/fact should have been created during the incognito turn.
    let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
    assert!(
        found.is_empty(),
        "incognito turn must not persist entities, got: {found:?}"
    );
}
#[tokio::test]
async fn test_non_incognito_allows_remember_tool_and_persists_fact_stream() {
    // Control: the same tool call persists a fact when not incognito,
    // proving the incognito guard is what prevents writes (issue #155).
    let tool_call = ToolCall {
        index: 0,
        id: "call_remember".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "remember".to_string(),
            arguments: serde_json::json!({
                "facts": [{
                    "classification": "Explicit",
                    "subject": "Incognito Test User",
                    "subject_type": "Person",
                    "relationship_type": "based_in",
                    "object": "London",
                    "object_is_entity": false,
                    "is_sensitive": false,
                    "categories": []
                }]
            })
            .to_string(),
        },
    };
    // The streaming endpoint reads from the mock's stream-response queue
    // (push_stream / StreamItem), not the blocking chat-message queue, so
    // queue a `remember` tool-call stream followed by the final reply.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::ToolCalls(vec![tool_call])),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .push_stream(vec![
                Ok(StreamItem::Text("Noted.".to_string())),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let kg = Arc::clone(&state.knowledge_graph);
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "remember that I am based in London",
        "incognito": false,
    }))
    .unwrap();
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

    // Drain the SSE response body so the spawned stream task completes the
    // `remember` tool execution (and fact persistence) before we assert.
    let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
    assert!(
        !found.is_empty(),
        "non-incognito turn should persist the entity/fact"
    );
}
