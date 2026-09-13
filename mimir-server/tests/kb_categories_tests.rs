mod common;
use common::*;

#[tokio::test]
async fn create_category_canonicalises_and_persists_name() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::json!({
        "id": 99_101,
        "name": "  Reviewers  "
    });
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/categories")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let category: mimir_api_types::CategoryResponse = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(category.name, "Reviewers");
    assert_eq!(
        state
            .knowledge_graph
            .get_category(category.id)
            .await
            .unwrap()
            .map(|c| c.name),
        Some("Reviewers".to_string())
    );
}

#[tokio::test]
async fn create_category_rejects_invalid_input() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let bodies = vec![
        serde_json::json!({ "id": 99_102, "name": "   " }),
        serde_json::json!({ "id": 99_102, "name": "Bad\nName" }),
        serde_json::json!({ "id": 0, "name": "Reviewers" }),
    ];
    for body in bodies {
        let response = app
            .clone()
            .oneshot(
                authed_request()
                    .method("POST")
                    .uri("/kb/categories")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(error["code"], "VALIDATION_ERROR");
    }

    assert!(
        state
            .knowledge_graph
            .get_category(99_102)
            .await
            .unwrap()
            .is_none()
    );
}
