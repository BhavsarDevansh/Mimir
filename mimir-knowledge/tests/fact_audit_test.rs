//! Audit log integration tests: insert, status change, and forget-cascade entries.

mod common;
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
// ---------------------------------------------------------------------------
// Audit log written on insert and status change
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_audit_log_written() {
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

    let log = kg.get_audit_log(fact.id).await.unwrap();
    assert!(log.iter().any(|e| e.change_type_id == 1));

    kg.update_fact_status(fact.id, FactStatus::Disputed, ChangedBy::User)
        .await
        .unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    assert!(log.iter().any(|e| e.change_type_id == 2));
}
// ---------------------------------------------------------------------------
// Forget cascade: status change to Disputed writes audit log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn forget_cascade_status_change_writes_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    // Parent fact with high confidence.
    let parent = kg
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

    // Non-inferred child with confidence that will drop below 0.20 when parent is removed.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
    )
    .bind(alice)
    .bind(2i16)
    .bind(london)
    .bind(0.8f32)
    .bind(FactStatus::Active as i16)
    .bind(false)
    .bind(0i32)
    .bind(false)
    .bind(false)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
         VALUES (?, ?, ?)",
    )
    .bind(parent.id)
    .bind(child.id)
    .bind(1i16)
    .execute(kg.pool())
    .await
    .unwrap();

    // Forget parent → child confidence recalculates to 0 (no parents left).
    // 0 < 0.20 triggers STATUS_CHANGE to Disputed.
    kg.forget_fact(parent.id, ChangedBy::User).await.unwrap();

    let child_alive = kg.get_fact(child.id).await.unwrap();
    assert!(child_alive.is_some());
    let child_alive = child_alive.unwrap();
    assert_eq!(child_alive.status().unwrap(), FactStatus::Disputed);

    let log = kg.get_audit_log(child.id).await.unwrap();
    let status_change = log.iter().find(|e| e.change_type_id == 2);
    assert!(
        status_change.is_some(),
        "Expected a STATUS_CHANGE audit log entry for cascade Disputed"
    );
    let entry = status_change.unwrap();
    assert!(entry.old_value.is_some());
    assert!(entry.new_value.is_some());
    assert_eq!(entry.changed_by_id, Some(2));
}
