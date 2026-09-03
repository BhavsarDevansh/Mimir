//! Query unit tests for `kg_search` tool logic.

use chrono::{TimeZone, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries::search::search_entities;

mod common;

#[tokio::test]
async fn test_kg_search_basic() {
    let tg = common::TestGraph::new().await;
    let london = tg.create_place("London").await;

    let f = NewFact {
        subject_id: london,
        relationship_type: "is_in".to_string(),
        object_id: None,
        object_literal: Some("United Kingdom".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f).await.unwrap();

    let results = search_entities(tg.kg.pool(), "London", None, 10)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entity.name, "London");
    assert!(!results[0].top_facts.is_empty());
}

#[tokio::test]
async fn test_kg_search_preserves_temporal_bounds() {
    let tg = common::TestGraph::new().await;
    let london = tg.create_place("London").await;

    let f = NewFact {
        subject_id: london,
        relationship_type: "has_event".to_string(),
        object_id: None,
        object_literal: Some("Property Check-In".to_string()),
        valid_from: Some(Utc.with_ymd_and_hms(2025, 7, 16, 0, 0, 0).unwrap()),
        valid_until: Some(Utc.with_ymd_and_hms(2025, 7, 20, 0, 0, 0).unwrap()),
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f).await.unwrap();

    let results = search_entities(tg.kg.pool(), "London", None, 10)
        .await
        .unwrap();
    let fact = &results[0].top_facts[0];

    assert_eq!(
        fact.valid_from,
        Some(Utc.with_ymd_and_hms(2025, 7, 16, 0, 0, 0).unwrap())
    );
    assert_eq!(
        fact.valid_until,
        Some(Utc.with_ymd_and_hms(2025, 7, 20, 0, 0, 0).unwrap())
    );
}

#[tokio::test]
async fn test_kg_search_entity_type_filter() {
    let tg = common::TestGraph::new().await;
    let _alice = tg.create_person("Alice").await;
    let _london = tg.create_place("London").await;

    let results = search_entities(tg.kg.pool(), "London", Some(EntityType::Place), 10)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entity.entity_type, "Place");
}

#[tokio::test]
async fn test_kg_search_empty_results() {
    let tg = common::TestGraph::new().await;
    let results = search_entities(tg.kg.pool(), "xyzabc123", None, 10)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_kg_search_fts5_injection() {
    let tg = common::TestGraph::new().await;
    let london = tg.create_place("London").await;

    let f = NewFact {
        subject_id: london,
        relationship_type: "is_in".to_string(),
        object_id: None,
        object_literal: Some("United Kingdom".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f).await.unwrap();

    // Positive match: normal search finds the seeded entity.
    let results = search_entities(tg.kg.pool(), "London", None, 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entity.name, "London");

    // Malicious payloads should return no results.
    let results = search_entities(tg.kg.pool(), "\" OR 1=1", None, 10)
        .await
        .unwrap();
    assert!(results.is_empty());

    let results = search_entities(tg.kg.pool(), "*", None, 10).await.unwrap();
    assert!(results.is_empty());

    let results = search_entities(tg.kg.pool(), "\"", None, 10).await.unwrap();
    assert!(results.is_empty());
}
