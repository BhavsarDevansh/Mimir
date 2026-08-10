//! Initial confidence values per source type.

mod common;
use chrono::{TimeZone, Utc};
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
// ---------------------------------------------------------------------------
// Confidence initial values per source type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn confidence_initial_values() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f_user = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert!((f_user.confidence - 1.0).abs() < f32::EPSILON);

    let f_inf = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "visited".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::Inference,
            connector_instance_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert!((f_inf.confidence - 0.0).abs() < f32::EPSILON);

    let f_conn = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "owns".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::Connector,
            connector_instance_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert!((f_conn.confidence - 0.80).abs() < f32::EPSILON);
}
