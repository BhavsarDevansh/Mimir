//! Temporal fact semantics: timelines, disputes, closure, boundary queries, and validation.

mod common;
use chrono::{TimeZone, Utc};
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;

// ---------------------------------------------------------------------------
// Temporal: non-overlapping ranges both Active
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_temporal_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let f1 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
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

    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
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

    assert_eq!(f1.status().unwrap(), FactStatus::Active);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
}

// ---------------------------------------------------------------------------
// Temporal: overlapping ranges → Disputed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_temporal_disputed() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let _f1 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
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

    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
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

    // With explicit replacement, f1 is Superseded and f2 is Active.
    let f1_updated = kg.get_fact(_f1.id).await.unwrap().unwrap();
    assert_eq!(f1_updated.status().unwrap(), FactStatus::Superseded);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
}

// ---------------------------------------------------------------------------
// Temporal: open-ended old + new explicit → old gets closed and Superseded, new Active
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_temporal_closure() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let f1 = kg
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

    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(kg.now()),
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

    assert_eq!(f2.status().unwrap(), FactStatus::Active);

    let old = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!(old.valid_until.is_some());
    assert_eq!(old.status().unwrap(), FactStatus::Superseded);
}

// ---------------------------------------------------------------------------
// Temporal: half-open boundary semantics in get_active_facts_at
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_active_facts_at_half_open_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let boundary = Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap();

    // f1: [2020-01-01, 2021-01-01) — ends exactly at boundary
    let _f1 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(boundary),
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

    // f2: [2021-01-01, 2022-01-01) — starts exactly at boundary
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(boundary),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
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

    let active = kg
        .get_active_facts_at(
            alice,
            kg.ensure_relationship_type("is_in").await.unwrap(),
            boundary,
        )
        .await
        .unwrap();

    // Half-open semantics: f1 ends at boundary, so it is NOT active.
    // f2 starts at boundary, so it IS active.
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, f2.id);
}

// ---------------------------------------------------------------------------
// Active status filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_active_facts_at_filters_by_active_status() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
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

    kg.update_fact_status(fact.id, FactStatus::Disputed, ChangedBy::User)
        .await
        .unwrap();

    let active = kg
        .get_active_facts_at(
            alice,
            kg.ensure_relationship_type("is_in").await.unwrap(),
            Utc::now(),
        )
        .await
        .unwrap();

    assert!(
        active.is_empty(),
        "Disputed facts should not appear in get_active_facts_at"
    );
}

// ---------------------------------------------------------------------------
// Temporal: automatic closure writes audit log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn automatic_closure_writes_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let now = kg.now();

    // Open-ended fact.
    let old_fact = kg
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

    // New fact with explicit start → should close old_fact at now().
    let _new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(now),
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

    let log = kg.get_audit_log(old_fact.id).await.unwrap();
    let closure_entry = log.iter().find(|e| e.change_type_id == 4);
    assert!(
        closure_entry.is_some(),
        "Expected an UPDATE audit log entry for automatic closure"
    );
    let entry = closure_entry.unwrap();
    assert!(entry.old_value.is_some());
    assert!(entry.new_value.is_some());
    assert_eq!(entry.changed_by_id, Some(2));
}

// ---------------------------------------------------------------------------
// Temporal: inverted range rejected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn insert_rejects_inverted_time_range() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let from = Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap();
    let until = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

    let result = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(from),
            valid_until: Some(until),
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
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("valid_from"));
    assert!(err.contains("valid_until"));
}
