mod common;
use common::*;

async fn stage_fact(state: &Arc<AppState>, predicate: &str) -> i64 {
    state
        .knowledge_graph
        .stage_unrecognized_fact(
            None,
            Some("test:1"),
            predicate,
            r#"{"object":"Acme Bank"}"#,
            None,
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn test_kb_staged_lists_mapped_and_rejects() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let id = stage_fact(&state, "owes").await;
    let leaf = state
        .knowledge_graph
        .get_relationship_type_id("has_event")
        .await
        .unwrap()
        .unwrap();
    let app = mimir_server::build_app(state.clone());

    let list_response = app
        .clone()
        .oneshot(
            authed_request()
                .uri("/kb/staged")
                .extension(loopback_connect_info())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: mimir_api_types::UnrecognizedFactListResponse =
        serde_json::from_slice(&body).unwrap();
    assert_eq!(list.total, 1);
    assert_eq!(list.items[0].id, id);
    assert_eq!(list.items[0].relationship_type_raw, "owes");

    let map_response = app
        .clone()
        .oneshot(
            authed_request()
                .method("POST")
                .uri(format!("/kb/staged/{id}/map"))
                .header("content-type", "application/json")
                .extension(loopback_connect_info())
                .body(Body::from(
                    serde_json::to_vec(&mimir_api_types::MapUnrecognizedFactRequest {
                        relationship_type_id: leaf,
                        note: Some("maps to has_event".to_string()),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(map_response.status(), StatusCode::OK);
    assert!(
        state
            .knowledge_graph
            .list_unrecognized_facts(Some("unmapped"))
            .await
            .unwrap()
            .is_empty()
    );

    let rejected_id = stage_fact(&state, "owes-money").await;
    let reject_response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri(format!("/kb/staged/{rejected_id}/reject"))
                .extension(loopback_connect_info())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reject_response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_kb_staged_rejects_non_loopback() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .uri("/kb/staged")
                .extension(non_loopback_connect_info())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
