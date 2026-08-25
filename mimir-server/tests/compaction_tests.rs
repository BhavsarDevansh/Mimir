//! Request-path session-compaction integration tests (issue #279, PR #505
//! review).

mod common;
use common::*;

/// Send more than `context.max_turns` turns while the idle-gated hooks are
/// held back by the cooldown, and assert the hard trim never drops turns
/// without first writing them to `sessions.summary` (PR #505 review).
#[tokio::test]
async fn hard_trim_compacts_synchronously_before_dropping_turns_during_idle_cooldown() {
    let mut config = Config::default();
    config.context.max_turns = 20;
    config.context.compaction.enabled = true;
    config.context.compaction.max_turns = 15;
    // A cooldown far beyond the test duration keeps the idle-gated
    // `remember.chat` hook pending, and the test harness does not register
    // `session.compaction` at all, so the burst simulates a user racing ahead
    // of the background compaction job.
    config.scheduler.cooldown_seconds = 3600;

    let mut builder = MockLlmClient::builder();
    // Turns 1..20 consume one chat response each.
    for i in 1..=20 {
        builder = builder.push_chat(format!("reply-{i}"), Usage::default());
    }
    // Turn 21 crosses the hard ceiling: the request path compacts
    // synchronously first (one summarisation call), then answers the turn.
    builder = builder.push_chat("Summarised turns 1-5", Usage::default());
    for i in 21..=25 {
        builder = builder.push_chat(format!("reply-{i}"), Usage::default());
    }
    let mock = Arc::new(builder.build());

    let (state, _temp) = test_state_with_config(mock, config).await;
    let app = mimir_server::build_app(state.clone());

    let mut session_id = None;
    for i in 1..=25 {
        let mut body = serde_json::json!({ "message": format!("turn-{i}") });
        if let Some(sid) = session_id {
            body["session_id"] = serde_json::json!(sid);
        }
        let body = serde_json::to_string(&body).unwrap();
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
        assert_eq!(response.status(), StatusCode::OK, "turn {i} must succeed");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat: ChatResponse = serde_json::from_slice(&bytes).unwrap();
        session_id.get_or_insert(chat.session_id);
    }

    let sid = session_id.expect("at least one session id");
    let session = state.context_manager.load_session(sid).await.unwrap();
    assert_eq!(
        session.summary.as_deref(),
        Some("Summarised turns 1-5"),
        "turns deleted by the hard trim must be written to sessions.summary"
    );
    assert!(session.compacted_at.is_some());

    let msgs = state.context_manager.export_messages(sid).await.unwrap();
    assert_eq!(msgs[0].role, "system", "system prompt stays first");
    assert_eq!(
        msgs[1].role, "user",
        "the summary is exported as clearly labelled non-system context"
    );
    assert!(msgs[1].content.contains("Summarised turns 1-5"));

    let persisted_users = msgs
        .iter()
        .filter(|m| m.role == "user" && !m.content.starts_with("Earlier conversation summary"))
        .collect::<Vec<_>>();
    assert_eq!(
        persisted_users.len(),
        20,
        "exactly the max_turns ceiling of turns remains"
    );
    assert_eq!(
        persisted_users[0].content, "turn-6",
        "the oldest summarised turns must be gone"
    );
}
