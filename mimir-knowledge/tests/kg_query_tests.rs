//! Query unit tests for `kg_query` tool logic.

use chrono::Utc;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries::fact::set_status;

mod common;

#[tokio::test]
async fn test_kg_query_happy_path() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;

    let new_fact = NewFact {
        subject_id: alice,
        relationship_type: "lives_in".to_string(),
        object_id: Some(london),
        object_literal: None,
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
    let fact = tg.kg.insert_fact(new_fact).await.unwrap();

    let facts = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        None,
        0.0,
        0,
        50,
    )
    .await
    .unwrap();

    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].id, fact.id);
    assert_eq!(facts[0].confidence, 0.9);
    assert_eq!(facts[0].object_id, Some(london));
    assert!(!facts[0].pending_confirmation);
}

#[tokio::test]
async fn test_kg_query_entity_not_found() {
    let tg = common::TestGraph::new().await;
    let facts = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        99999,
        None,
        0.0,
        0,
        50,
    )
    .await
    .unwrap();
    assert!(facts.is_empty());
}

#[tokio::test]
async fn test_kg_query_predicate_filter() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;
    let book = tg.create_activity("Reading").await;

    let f1 = NewFact {
        subject_id: alice,
        relationship_type: "lives_in".to_string(),
        object_id: Some(london),
        object_literal: None,
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
    let f2 = NewFact {
        subject_id: alice,
        relationship_type: "enjoys".to_string(),
        object_id: Some(book),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.8),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f1).await.unwrap();
    tg.kg.insert_fact(f2).await.unwrap();

    let pred_id = tg.kg.ensure_relationship_type("lives_in").await.unwrap();
    let facts = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        Some(pred_id),
        0.0,
        0,
        50,
    )
    .await
    .unwrap();

    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].relationship_type_id, pred_id);
}

#[tokio::test]
async fn test_kg_query_confidence_filter() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;

    let f1 = NewFact {
        subject_id: alice,
        relationship_type: "lives_in".to_string(),
        object_id: Some(london),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.3),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    let f2 = NewFact {
        subject_id: alice,
        relationship_type: "visited".to_string(),
        object_id: Some(london),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.8),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f1).await.unwrap();
    tg.kg.insert_fact(f2).await.unwrap();

    let facts = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        None,
        0.5,
        0,
        50,
    )
    .await
    .unwrap();

    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].confidence, 0.8);
}

#[tokio::test]
async fn test_kg_query_pagination() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;

    for i in 0..60 {
        let f = NewFact {
            subject_id: alice,
            relationship_type: format!("predicate_{}", i),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.5 + (i as f32 / 1000.0)),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        };
        tg.kg.insert_fact(f).await.unwrap();
    }

    let page1 = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        None,
        0.0,
        0,
        50,
    )
    .await
    .unwrap();
    assert_eq!(page1.len(), 50);

    let page2 = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        None,
        0.0,
        50,
        50,
    )
    .await
    .unwrap();
    assert_eq!(page2.len(), 10);
}

#[tokio::test]
async fn test_kg_query_excludes_pending() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;

    let f = NewFact {
        subject_id: alice,
        relationship_type: "lives_in".to_string(),
        object_id: Some(london),
        object_literal: None,
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

    // Mark as pending via raw SQL.
    sqlx::query("UPDATE facts SET pending_confirmation = TRUE WHERE subject_id = ?")
        .bind(alice)
        .execute(tg.kg.pool())
        .await
        .unwrap();

    let facts = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        None,
        0.0,
        0,
        50,
    )
    .await
    .unwrap();

    assert!(facts.is_empty());
}

#[tokio::test]
async fn test_kg_query_excludes_superseded_forgotten() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;

    let f1 = NewFact {
        subject_id: alice,
        relationship_type: "lives_in".to_string(),
        object_id: Some(london),
        object_literal: None,
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
    let fact = tg.kg.insert_fact(f1).await.unwrap();

    set_status(
        tg.kg.pool(),
        fact.id,
        FactStatus::Superseded,
        Utc::now(),
        mimir_knowledge::models::audit_log::ChangedBy::System,
    )
    .await
    .unwrap();

    let facts = mimir_knowledge::queries::fact::get_facts_by_subject_filtered(
        tg.kg.pool(),
        alice,
        None,
        0.0,
        0,
        50,
    )
    .await
    .unwrap();

    assert!(facts.is_empty());
}

#[tokio::test]
async fn test_kg_query_input_too_long() {
    use mimir_core::tools::{Tool, ToolError};

    let tg = common::TestGraph::new().await;
    let tool = mimir_knowledge::KgQueryTool::new(std::sync::Arc::new(tg.kg));

    let long_name = "a".repeat(300);
    let args = serde_json::json!({ "entity_name": long_name });
    let result = tool.execute(args).await;

    assert!(
        matches!(result, Err(ToolError::InvalidArguments { .. })),
        "expected InvalidArguments for 300-char entity_name, got {:?}",
        result
    );
}
