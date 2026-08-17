//! API authentication tests (issue #281): every route except `GET /health`
//! must reject requests that do not present the daemon's bearer token.
mod common;
use common::*;

#[tokio::test]
async fn test_status_rejects_missing_token() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );
}

#[tokio::test]
async fn test_status_rejects_wrong_token() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .header("Authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );
}

#[tokio::test]
async fn test_status_rejects_malformed_authorization() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    // A non-bearer scheme must not authenticate, even with the right secret.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .header("Authorization", format!("Basic {TEST_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );
}

#[tokio::test]
async fn test_status_accepts_lowercase_bearer_scheme() {
    // RFC 7235 auth schemes are case-insensitive; the CLI always sends
    // `Bearer`, but a hand-written client may not.
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .header("Authorization", format!("bearer {TEST_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_status_accepts_tab_separated_scheme() {
    // RFC 7235 allows SP or HTAB between the scheme and the credentials.
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .header("Authorization", format!("Bearer\t{TEST_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_status_accepts_correct_token() {
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
async fn test_chat_rejects_missing_token() {
    // A state-changing route must be protected too: forging chat turns is
    // part of the issue's threat model.
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/chat")
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_kb_query_rejects_missing_token() {
    // Reading the knowledge graph without the token must fail: the issue's
    // threat model explicitly covers local processes reading the graph.
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/kb/query?entity=anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_health_stays_unauthenticated() {
    // `/health` is the daemon-guard liveness probe and must stay open so the
    // CLI can detect a running daemon before it has a token.
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
