mod common;
use common::*;

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
            .push_chat_message(extraction_message(), Usage::default())
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
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
    // Issue #279: incognito must also never enqueue session compaction — the
    // turn is not persisted, so there is nothing to summarise.
    assert_eq!(
        state
            .hook_engine
            .pending_depth_for("session.compaction")
            .await,
        0,
        "incognito turns must never enqueue the compaction hook"
    );
    // Give any (incorrect) hook dispatch time to run to completion: the mock
    // is configured with an extraction response, so a fired hook would have
    // persisted the `Devansh` fact by the time the queue drains.
    wait_for_chat_hook_idle(&state).await;
    assert!(
        !has_favourite_colour(&state).await,
        "incognito turn must not persist facts (the user entity itself is created at daemon start)"
    );
    let sessions = state.context_manager.list_sessions().await.unwrap();
    assert!(
        sessions.is_empty(),
        "incognito turns must not create or compact any session"
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
            .push_chat_message(extraction_message(), Usage::default())
            .build(),
    );
    let (state, _temp) = test_state_with_config(mock, fast_learning_config()).await;
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
    wait_for_chat_hook_idle(&state).await;
    assert!(
        !has_favourite_colour(&state).await,
        "incognito turn must not persist facts (the user entity itself is created at daemon start)"
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
