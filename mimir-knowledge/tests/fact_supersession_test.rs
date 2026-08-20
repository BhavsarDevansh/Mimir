//! Explicit-fact supersession and non-superseding atemporal facts.

mod common;
use chrono::{TimeZone, Utc};
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
// ---------------------------------------------------------------------------
// Explicit replacement (supersession)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn explicit_replaces_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    // Old explicit fact.
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

    // New explicit fact with temporal overlap.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
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

    // Old fact is Superseded.
    let old_updated = kg.get_fact(old_fact.id).await.unwrap().unwrap();
    assert_eq!(old_updated.status().unwrap(), FactStatus::Superseded);
    assert!((old_updated.confidence - 1.0).abs() < f32::EPSILON);

    // New fact is Active.
    assert_eq!(new_fact.status().unwrap(), FactStatus::Active);
    assert!((new_fact.confidence - 1.0).abs() < f32::EPSILON);

    // Supersedes edge exists.
    let edge_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fact_dependencies \
         WHERE parent_fact_id = ? AND child_fact_id = ? AND relation_type_id = ?",
    )
    .bind(old_fact.id)
    .bind(new_fact.id)
    .bind(3i16) // Supersedes
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(edge_count, 1);

    // Audit log has STATUS_CHANGE for old fact.
    let log = kg.get_audit_log(old_fact.id).await.unwrap();
    let status_change = log.iter().find(|e| e.change_type_id == 2);
    assert!(
        status_change.is_some(),
        "Expected STATUS_CHANGE audit entry for superseded fact"
    );
}

#[tokio::test]
async fn explicit_replaces_inferred() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let located_in_id = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .unwrap();

    // Inferred fact.
    let old_fact: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id, subject_id, relationship_type_id, object_id, object_literal, valid_from, valid_until, confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
    )
    .bind(alice)
    .bind(located_in_id)
    .bind(london)
    .bind(0.5f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(1i32)
    .bind(false)
    .bind(false)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    // Explicit replacement.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "located_in".to_string(),
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

    let old_updated = kg.get_fact(old_fact.id).await.unwrap().unwrap();
    assert_eq!(old_updated.status().unwrap(), FactStatus::Superseded);
    assert_eq!(new_fact.status().unwrap(), FactStatus::Active);
}

#[tokio::test]
async fn explicit_replaces_connector() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    // Connector-extracted fact.
    let old_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
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

    // Explicit replacement with temporal overlap.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap()),
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

    let old_updated = kg.get_fact(old_fact.id).await.unwrap().unwrap();
    assert_eq!(old_updated.status().unwrap(), FactStatus::Superseded);
    assert_eq!(new_fact.status().unwrap(), FactStatus::Active);
}

#[tokio::test]
async fn explicit_no_overlap_no_supersession() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    // Explicit fact with bounded temporal range.
    let old_fact = kg
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

    // New explicit fact with NON-overlapping range.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
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

    // Both remain Active because ranges do not overlap.
    assert_eq!(old_fact.status().unwrap(), FactStatus::Active);
    assert_eq!(new_fact.status().unwrap(), FactStatus::Active);
}

#[tokio::test]
async fn explicit_replaces_already_superseded_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;
    let berlin = create_place(&kg, "Berlin").await;

    // First explicit fact.
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

    // Second explicit fact replaces first.
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
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

    // Third explicit fact replaces second; first is already Superseded.
    let f3 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(berlin),
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

    let f1_now = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert_eq!(f1_now.status().unwrap(), FactStatus::Superseded);

    let f2_now = kg.get_fact(f2.id).await.unwrap().unwrap();
    assert_eq!(f2_now.status().unwrap(), FactStatus::Superseded);

    assert_eq!(f3.status().unwrap(), FactStatus::Active);

    // Only one Supersedes edge from f1 (to f2), not duplicated by f3.
    let f1_edges: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM fact_dependencies WHERE parent_fact_id = ?")
            .bind(f1.id)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(f1_edges, 1);
}
// ---------------------------------------------------------------------------
// Multiple atemporal facts with same subject+pred but different objects
// should NOT supersede each other (regression test for hobby-loss bug).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_atemporal_facts_different_objects_all_persist() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;

    // Insert three hobbies with no temporal bounds — all should survive.
    let hobbies = ["Geopolitics", "Software Development", "tinkering"];
    let mut fact_ids = Vec::new();
    for hobby in &hobbies {
        let f = kg
            .insert_fact(NewFact {
                subject_id: alice,
                relationship_type: "hobby".to_string(),
                object_id: None,
                object_literal: Some(hobby.to_string()),
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
        fact_ids.push(f.id);
        assert_eq!(f.status().unwrap(), FactStatus::Active);
    }

    // All three should still be active.
    for &fid in &fact_ids {
        let f = kg.get_fact(fid).await.unwrap().unwrap();
        assert_eq!(f.status().unwrap(), FactStatus::Active);
    }

    // Query by subject+pred should return all three.
    let hobby_rt_id = kg.ensure_relationship_type("hobby").await.unwrap();
    let all_facts = kg
        .get_facts_by_subject_and_predicate(alice, hobby_rt_id)
        .await
        .unwrap();
    let active: Vec<_> = all_facts
        .into_iter()
        .filter(|f| f.status() == Some(FactStatus::Active))
        .collect();
    assert_eq!(active.len(), 3);
}
