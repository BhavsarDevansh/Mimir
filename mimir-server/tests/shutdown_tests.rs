mod common;
use common::*;

#[tokio::test]
async fn test_stop_returns_ok() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stop")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
#[tokio::test]
async fn test_stop_rejects_non_loopback() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stop")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [192, 168, 1, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn test_stop_handler_fires_shutdown_trigger() {
    use std::time::Duration;

    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    // Subscribe *before* issuing the request so the trigger is observed.
    let mut shutdown_rx = state.shutdown_tx.subscribe();
    assert!(
        !*shutdown_rx.borrow_and_update(),
        "shutdown trigger should be idle before /stop"
    );

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/stop")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The handler delays the send by STOP_DELAY_MS; allow up to 2 s.
    let observed = tokio::time::timeout(Duration::from_secs(2), shutdown_rx.changed()).await;
    assert!(
        observed.is_ok(),
        "shutdown trigger did not fire after /stop"
    );
    assert!(
        *shutdown_rx.borrow(),
        "trigger value must be true after /stop"
    );
}
