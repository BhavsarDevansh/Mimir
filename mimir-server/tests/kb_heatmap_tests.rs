//! `GET /kb/heatmap` route tests (issue #69).

mod common;
use common::*;

use chrono::Utc;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries;

async fn seed_fact(state: &AppState, entity_name: &str, relationship_type: &str, confidence: f32) {
    let entity = state
        .knowledge_graph
        .create_entity(entity_name, EntityType::Person, &[])
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type(relationship_type)
        .await
        .unwrap();
    queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &NewFact {
            subject_id: entity.id,
            relationship_type: relationship_type.to_string(),
            object_id: None,
            object_literal: Some("value".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        confidence,
        Utc::now(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_kb_heatmap_returns_aggregates() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

    seed_fact(&state, "Alice", "works_at", 0.95).await;
    seed_fact(&state, "Bob", "works_at", 0.6).await;
    seed_fact(&state, "Bob", "lives_in", 0.3).await;

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/heatmap")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::HeatmapResponse = serde_json::from_slice(&body).unwrap();

    assert_eq!(resp.facts, 3);
    assert_eq!(resp.entities, 2);
    assert!((resp.avg_confidence - ((0.95 + 0.6 + 0.3) / 3.0)).abs() < 1e-4);

    let top: Vec<(&str, i64)> = resp
        .top_entities
        .iter()
        .map(|e| (e.name.as_str(), e.count))
        .collect();
    assert_eq!(top, vec![("Bob", 2), ("Alice", 1)]);

    let predicates: Vec<(&str, i64)> = resp
        .predicates
        .iter()
        .map(|p| (p.name.as_str(), p.count))
        .collect();
    assert_eq!(predicates, vec![("works_at", 2), ("lives_in", 1)]);

    let bands: Vec<(String, i64)> = resp
        .confidence_bands
        .iter()
        .map(|b| (b.label.clone(), b.count))
        .collect();
    assert_eq!(
        bands,
        vec![
            ("explicit (1.0)".to_string(), 0),
            ("connector (0.7-0.9)".to_string(), 1),
            ("inference (0.4-0.7)".to_string(), 1),
            ("casual (<0.4)".to_string(), 1)
        ]
    );

    let current_month = Utc::now().format("%Y-%m").to_string();
    let temporal: Vec<(String, i64)> = resp
        .temporal
        .iter()
        .map(|t| (t.period.clone(), t.count))
        .collect();
    assert_eq!(temporal, vec![(current_month, 3)]);
}

#[tokio::test]
async fn test_kb_heatmap_empty_graph() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/heatmap")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::HeatmapResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp.facts, 0);
    assert_eq!(resp.entities, 0);
    assert_eq!(resp.avg_confidence, 0.0);
    assert!(resp.top_entities.is_empty());
    assert!(resp.confidence_bands.iter().all(|b| b.count == 0));
}
