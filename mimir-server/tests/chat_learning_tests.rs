mod common;
use common::*;

/// Config with zero debounce/cooldown so the `remember.chat` hook dispatches
/// immediately after the turn instead of waiting for the production windows.
fn fast_learning_config() -> Config {
    let mut config = Config::default();
    config.identity.name = "Devansh".to_string();
    config.agent.remember_debounce_seconds = 0;
    config.scheduler.cooldown_seconds = 0;
    config
}

/// The extraction response the `remember.chat` hook's Librarian pipeline
/// consumes: a `remember` tool call carrying one fact.
fn extraction_message() -> Message {
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
    Message {
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
    }
}

/// Poll until the KG holds `favourite_colour=blue` for the user, or fail
/// after a timeout.
async fn wait_for_favourite_colour(state: &Arc<AppState>) -> bool {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let search = state
            .knowledge_graph
            .search_entities("Devansh", 1)
            .await
            .unwrap();
        if let Some(result) = search.first() {
            let facts = state
                .knowledge_graph
                .get_facts_by_subject(result.entity.id, 100)
                .await
                .unwrap();
            for fact in &facts {
                let pred = state
                    .knowledge_graph
                    .relationship_type_name(fact.relationship_type_id)
                    .await;
                if pred.as_deref() == Some("favourite_colour")
                    && fact.object_literal.as_deref() == Some("blue")
                {
                    return true;
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn test_chat_extracts_facts_after_response() {
    // Hook-driven learning (issue #386): the completed turn is enqueued for
    // the debounced `remember.chat` hook, which runs the Librarian pipeline
    // in the background — the conversational LLM never calls `remember`.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Got it!", Usage::default())
            .push_chat_message(extraction_message(), Usage::default())
            .build(),
    );

    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
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
    assert!(
        wait_for_favourite_colour(&state).await,
        "expected favourite_colour=blue fact to be extracted within 5s"
    );
}

#[tokio::test]
async fn test_remember_tool_is_not_registered() {
    // Issue #386: the `remember` tool is removed from the registry — the
    // conversational LLM must never be able to write facts directly.
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    assert!(
        state.tool_registry.get("remember").is_none(),
        "the remember tool must not be registered"
    );
}

#[tokio::test]
async fn test_incognito_turn_enqueues_no_hook_and_writes_no_facts() {
    // Incognito stays a hard no-persistence guarantee (issue #155): the
    // turn is never enqueued for the `remember.chat` hook.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Noted.", Usage::default())
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
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

    assert_eq!(
        state.hook_engine.pending_depth_for("remember.chat").await,
        0,
        "incognito turns must never enqueue the chat hook"
    );
    let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
    assert!(
        found.is_empty(),
        "incognito turn must not persist entities, got: {found:?}"
    );
}

#[tokio::test]
async fn test_non_incognito_turn_enqueues_hook_and_persists_fact() {
    // Control: the same turn enqueues the hook when not incognito, proving
    // the incognito guard is what prevents learning (issue #155).
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Noted.", Usage::default())
            .push_chat_message(extraction_message(), Usage::default())
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::to_string(&serde_json::json!({
        "message": "My favourite colour is blue.",
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

    assert!(
        wait_for_favourite_colour(&state).await,
        "non-incognito turn should persist the entity/fact via the hook"
    );
}

#[tokio::test]
async fn test_incognito_stream_enqueues_no_hook_and_writes_no_facts() {
    // The streaming path honours the same incognito contract: no hook
    // instance is enqueued and no facts are persisted.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("Noted.".to_string())),
                Ok(StreamItem::Usage(Usage::default())),
            ])
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
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

    assert_eq!(
        state.hook_engine.pending_depth_for("remember.chat").await,
        0,
        "incognito stream turns must never enqueue the chat hook"
    );
    let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
    assert!(
        found.is_empty(),
        "incognito turn must not persist entities, got: {found:?}"
    );
}

#[tokio::test]
async fn test_non_incognito_stream_enqueues_hook_and_persists_fact() {
    // Control: the streaming path enqueues the hook when not incognito.
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

    let body = serde_json::to_string(&serde_json::json!({
        "message": "My favourite colour is blue.",
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
    // turn (and the hook enqueue) before we assert.
    let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    assert!(
        wait_for_favourite_colour(&state).await,
        "non-incognito stream turn should persist the entity/fact via the hook"
    );
}
