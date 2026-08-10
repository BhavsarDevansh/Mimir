//! Cascade forget semantics and trash payload integration tests.

mod common;
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
// ---------------------------------------------------------------------------
// Cascade forget: orphan inferred fact deleted when only dependency removed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cascade_forget_orphan() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let parent = kg
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

    // Create an inferred child fact manually.
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
    .bind(0.5f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(1i32)
    .bind(false)
    .bind(false)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    // Link child to parent.
    sqlx::query(
        "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
         VALUES (?, ?, ?)",
    )
    .bind(parent.id)
    .bind(child.id)
    .bind(1i16) // InferredFrom
    .execute(kg.pool())
    .await
    .unwrap();

    // Forget parent → child should also be forgotten (orphan).
    kg.forget_fact(parent.id, ChangedBy::User).await.unwrap();

    let parent_gone = kg.get_fact(parent.id).await.unwrap();
    let child_gone = kg.get_fact(child.id).await.unwrap();
    assert!(parent_gone.is_none());
    assert!(child_gone.is_none());
}
// ---------------------------------------------------------------------------
// Cascade forget: inferred child survives when other parents remain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cascade_forget_survives() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let parent_a = kg
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

    let parent_b = kg
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

    // Inferred child with two parents.
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
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(1i32)
    .bind(false)
    .bind(false)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
         VALUES (?, ?, ?)",
    )
    .bind(parent_a.id)
    .bind(child.id)
    .bind(1i16)
    .execute(kg.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
         VALUES (?, ?, ?)",
    )
    .bind(parent_b.id)
    .bind(child.id)
    .bind(1i16)
    .execute(kg.pool())
    .await
    .unwrap();

    // Forget one parent → child should survive.
    kg.forget_fact(parent_a.id, ChangedBy::User).await.unwrap();

    let child_alive = kg.get_fact(child.id).await.unwrap();
    assert!(child_alive.is_some());
}
// ---------------------------------------------------------------------------
// Trash contains JSON payload of forgotten fact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trash_contains_payload() {
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

    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();

    let (payload,): (String,) = sqlx::query_as(
        "SELECT payload FROM trash WHERE original_table = 'facts' AND original_id = ?",
    )
    .bind(fact.id)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    assert!(payload.contains("\"fact\""));
    assert!(payload.contains("\"sources\""));
}
