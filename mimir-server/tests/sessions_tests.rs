mod common;
use common::*;

#[tokio::test]
async fn test_sessions_returns_list() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .uri("/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: Vec<mimir_api_types::SessionSummary> = serde_json::from_slice(&body_bytes).unwrap();
    assert!(list.is_empty());
}
#[tokio::test]
async fn test_session_messages_returns_messages() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    let sid = state
        .context_manager
        .create_session("you are a test assistant")
        .await
        .unwrap();
    state
        .context_manager
        .add_user_message(sid, "hello")
        .await
        .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .uri(format!("/sessions/{}/messages", sid))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::SessionMessagesResponse =
        serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp.session_id, sid);
    assert_eq!(resp.messages.len(), 2);
    assert_eq!(resp.messages[0].role, "system");
    assert_eq!(resp.messages[1].role, "user");
    assert_eq!(resp.messages[1].content, "hello");
}
#[tokio::test]
async fn test_session_messages_unknown_session_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .uri("/sessions/not-a-session/messages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
