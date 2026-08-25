mod common;
use common::*;

async fn insert_pending_fact(state: &Arc<AppState>, object: &str) -> i32 {
    use mimir_knowledge::extract::{
        Classification, ExtractedFact, RememberOutput, process_remember_output,
    };
    let outcome = process_remember_output(
        &state.knowledge_graph,
        RememberOutput {
            facts: vec![ExtractedFact {
                classification: Classification::Explicit,
                subject: "Devansh".to_string(),
                subject_type: "Person".to_string(),
                relationship_type: "allergy".to_string(),
                object: object.to_string(),
                object_is_entity: false,
                object_type: None,
                temporal: None,
                is_sensitive: true,
                correction_scope: None,
                // Category 230 = Allergies & Intolerances, a sensitive
                // catalogue category. The #142 sensitivity AND-gate
                // requires the LLM flag *and* a sensitive category/keyword,
                // so without this the fact is correctly narrowed to
                // non-sensitive and never reaches pending_confirmation.
                categories: vec!["230".to_string()],
                recurrence: None,
                requires_user_action: None,
                location: None,
            }],
        },
    )
    .await
    .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.pending_confirmation[0].fact_id
}
#[tokio::test]
async fn test_kb_pending_lists_pending_facts() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let fact_id = insert_pending_fact(&state, "peanuts").await;

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .uri("/kb/pending")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::PendingListResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp.total, 1);
    assert_eq!(resp.facts[0].fact_id, fact_id);
    assert_eq!(resp.facts[0].subject, "Devansh");
    assert_eq!(resp.facts[0].predicate, "allergy");
    assert_eq!(resp.facts[0].object.as_deref(), Some("peanuts"));
}
#[tokio::test]
async fn test_kb_confirm_returns_active_fact() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let fact_id = insert_pending_fact(&state, "shellfish").await;

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri(format!("/kb/facts/{fact_id}/confirm"))
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::ConfirmFactResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp.fact.id, fact_id);
    assert_eq!(resp.fact.status, "Active");
    assert!((resp.fact.confidence - 1.0).abs() < f32::EPSILON);

    // No longer pending.
    let pending = state.knowledge_graph.list_pending_facts().await.unwrap();
    assert!(pending.is_empty());
}
#[tokio::test]
async fn test_kb_confirm_non_pending_returns_bad_request() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let fact_id = insert_pending_fact(&state, "pollen").await;
    state.knowledge_graph.confirm_fact(fact_id).await.unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri(format!("/kb/facts/{fact_id}/confirm"))
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn test_kb_reject_deletes_fact_and_returns_204() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let fact_id = insert_pending_fact(&state, "latex").await;

    let app = mimir_server::build_app(state.clone());
    let body = serde_json::to_string(&serde_json::json!({
        "reason": "entered in error"
    }))
    .unwrap();
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri(format!("/kb/facts/{fact_id}/reject"))
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Fact hard-deleted.
    assert!(
        state
            .knowledge_graph
            .get_fact(fact_id)
            .await
            .unwrap()
            .is_none()
    );

    // Audit log carries the user reason.
    let audit = state.knowledge_graph.get_audit_log(fact_id).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.reason.as_deref() == Some("User rejected sensitive fact: entered in error"))
    );
}
#[tokio::test]
async fn test_kb_reject_empty_body_returns_204() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let fact_id = insert_pending_fact(&state, "dust").await;

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri(format!("/kb/facts/{fact_id}/reject"))
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
#[tokio::test]
async fn test_kb_pending_rejects_non_loopback() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .uri("/kb/pending")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([192, 168, 1, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn test_kb_confirm_rejects_non_loopback() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/facts/1/confirm")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([192, 168, 1, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn test_kb_reject_rejects_non_loopback() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/facts/1/reject")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([192, 168, 1, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
