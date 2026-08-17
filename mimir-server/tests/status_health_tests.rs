mod common;
use common::*;

#[tokio::test]
async fn test_status_returns_ok() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(authed_request().uri("/status").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
#[tokio::test]
async fn test_health_returns_ok_without_llm() {
    // `/health` is the cheap liveness probe used by the daemon guard, so it
    // must never touch the LLM backend (which would make the 500ms probe
    // time out on a healthy-but-slow provider).
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(authed_request().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(bytes.is_empty(), "health endpoint should return no body");
}
#[tokio::test]
async fn test_status_returns_queue_depths() {
    let mock = Arc::new(
        MockLlmClient::builder()
            .user_queue_depth(2)
            .system_queue_depth(1)
            .worker_threads(4)
            .build(),
    );
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(authed_request().uri("/status").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status: StatusResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status.queue_depth_user, 2);
    assert_eq!(status.queue_depth_system, 1);
    assert_eq!(status.worker_threads, 4);
}
