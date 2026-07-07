//! Integration tests for the fact management subsystem (#50).

use chrono::{TimeZone, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::ConnectorType;

use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn create_person(kg: &KnowledgeGraph, name: &str) -> i32 {
    let entity = kg
        .create_entity(name, EntityType::Person, &[])
        .await
        .unwrap();
    entity.id
}

async fn create_place(kg: &KnowledgeGraph, name: &str) -> i32 {
    let entity = kg
        .create_entity(name, EntityType::Place, &[])
        .await
        .unwrap();
    entity.id
}

// ---------------------------------------------------------------------------
// CRUD roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_crud_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let new_fact = NewFact {
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
    };

    let fact = kg.insert_fact(new_fact.clone()).await.unwrap();
    assert_eq!(fact.subject_id, alice);
    assert_eq!(fact.relationship_type_id, 1i16);
    assert_eq!(fact.status().unwrap(), FactStatus::Active);

    // Read back
    let fetched = kg.get_fact(fact.id).await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, fact.id);

    // Update status
    let updated = kg
        .update_fact_status(fact.id, FactStatus::Disputed, ChangedBy::User)
        .await
        .unwrap();
    assert_eq!(updated.status().unwrap(), FactStatus::Disputed);

    // Update valid_until
    let until = Utc.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap();
    let updated = kg
        .update_fact_valid_until(fact.id, Some(until), ChangedBy::User)
        .await
        .unwrap();
    assert_eq!(updated.valid_until, Some(until));

    // Forget
    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();
    let gone = kg.get_fact(fact.id).await.unwrap();
    assert!(gone.is_none());
}

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
// Predicate id lookup roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_relationship_type_id_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let dev = create_person(&kg, "Developer").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "works_as".to_string(),
            object_id: Some(dev),
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

    assert_eq!(fact.relationship_type_id, 4i16);
    assert_eq!(
        kg.relationship_type_name(fact.relationship_type_id)
            .await
            .unwrap(),
        "works_as"
    );

    let by_predicate = kg
        .get_facts_by_relationship_type(kg.ensure_relationship_type("works_as").await.unwrap(), 10)
        .await
        .unwrap();
    assert!(by_predicate.iter().any(|f| f.id == fact.id));
}

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
// Source row attached on insert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_source_attached() {
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

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sources WHERE fact_id = ?")
        .bind(fact.id)
        .fetch_one(kg.pool())
        .await
        .unwrap();

    assert_eq!(count, 1);
}

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

// ---------------------------------------------------------------------------
// Enum mapping: unknown IDs return None
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_status_id_returns_none() {
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

    // Insert a dummy status ID that has no Rust enum mapping.
    sqlx::query("INSERT INTO fact_statuses (id, name) VALUES (?, ?)")
        .bind(999i16)
        .bind("UnknownStatus")
        .execute(kg.pool())
        .await
        .unwrap();

    // Update the fact to reference the unknown status.
    sqlx::query("UPDATE facts SET fact_status_id = ? WHERE id = ?")
        .bind(999i16)
        .bind(fact.id)
        .execute(kg.pool())
        .await
        .unwrap();

    let fetched = kg.get_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(fetched.status(), None);
    assert_eq!(fetched.fact_status_id, 999);
}

#[tokio::test]
async fn unknown_relationship_type_id_returns_none() {
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

    let fetched = kg.get_fact(fact.id).await.unwrap().unwrap();
    // is_in relationship_type_id is seeded as 1, not 4 — assert name instead.
    assert_eq!(
        kg.relationship_type_name(fetched.relationship_type_id)
            .await,
        Some("is_in".to_string())
    );

    // Assert that an uninserted predicate ID returns None.
    assert_eq!(kg.relationship_type_name(999i16).await, None);
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

    // Inferred fact.
    let old_fact: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id, subject_id, relationship_type_id, object_id, object_literal, valid_from, valid_until, confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
    )
    .bind(alice)
    .bind(1i16)
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

// ---------------------------------------------------------------------------
// Corroboration detection (#79)
//
// When a new non-explicit fact covers the same claim as an existing fact
// (same subject + predicate + object, temporally overlapping), the existing
// fact receives a new source row and a +0.05 confidence boost (capped at
// 0.95 for non-explicit, non-inferred facts). No new facts row is created.
// ---------------------------------------------------------------------------

/// Build a connector-sourced `NewFact` with distinct provenance so two facts
/// are independent sources (required to clear the `sources` UNIQUE index).
#[allow(clippy::too_many_arguments)]
async fn connector_new_fact(
    kg: &KnowledgeGraph,
    subject_id: i32,
    predicate: &str,
    object_id: Option<i32>,
    object_literal: Option<String>,
    valid_from: Option<chrono::DateTime<Utc>>,
    valid_until: Option<chrono::DateTime<Utc>>,
    connector_id: &str,
    raw_reference: &str,
) -> NewFact {
    // Each distinct `connector_id` label is a distinct registered Photos
    // connector instance (default reliability 0.80), so two facts with
    // different labels are independent sources for corroboration.
    let instance_id = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Photos,
            slug: connector_id.to_string(),
            backend: "test".to_string(),
            display_name: connector_id.to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap()
        .id;
    NewFact {
        subject_id,
        relationship_type: predicate.to_string(),
        object_id,
        object_literal,
        valid_from,
        valid_until,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_id),
        connector_type: Some(ConnectorType::Photos),
        raw_reference: Some(raw_reference.to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    }
}

async fn source_count(kg: &KnowledgeGraph, fact_id: i32) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sources WHERE fact_id = ?")
        .bind(fact_id)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    count
}

async fn audit_count(kg: &KnowledgeGraph, fact_id: i32, change_type_id: i16) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM fact_audit_log WHERE fact_id = ? AND change_type_id = ?",
    )
    .bind(fact_id)
    .bind(change_type_id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    count
}

fn dt(y: i32, m: u32, d: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
}

#[tokio::test]
async fn corroboration_adds_source_not_new_fact() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "gmail",
                "msg-1",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(source_count(&kg, f1.id).await, 1);

    // Second independent connector source, same claim, temporally overlapping.
    let corroborated = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 6, 1)),
                None,
                "calendar",
                "event-1",
            )
            .await,
        )
        .await
        .unwrap();

    // No new facts row; the existing fact is returned.
    assert_eq!(corroborated.id, f1.id);
    assert_eq!(source_count(&kg, f1.id).await, 2);

    let (facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? AND object_id = ?")
            .bind(alice)
            .bind(london)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(facts, 1);
}

#[tokio::test]
async fn corroboration_boosts_confidence_capped_at_ninety_five() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    // Connector default confidence is 0.80.
    assert!((f1.confidence - 0.80).abs() < 1e-6);

    for (cid, rid) in [("s2", "r2"), ("s3", "r3"), ("s4", "r4"), ("s5", "r5")] {
        kg.insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                cid,
                rid,
            )
            .await,
        )
        .await
        .unwrap();
    }

    let final_fact = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!(
        (final_fact.confidence - 0.95).abs() < 1e-6,
        "expected 0.95, got {}",
        final_fact.confidence
    );
}

#[tokio::test]
async fn non_overlapping_temporal_ranges_stay_separate() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                Some(dt(2022, 1, 1)),
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    // Disjoint range — separate fact, no corroboration.
    let f2 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2023, 1, 1)),
                Some(dt(2024, 1, 1)),
                "s2",
                "r2",
            )
            .await,
        )
        .await
        .unwrap();

    assert_ne!(f1.id, f2.id);
    assert_eq!(f1.status().unwrap(), FactStatus::Active);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
    assert_eq!(source_count(&kg, f1.id).await, 1);
    assert_eq!(source_count(&kg, f2.id).await, 1);

    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((f1_after.confidence - 0.80).abs() < 1e-6);
}

#[tokio::test]
async fn explicit_new_overlapping_connector_supersedes_not_corroborates() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(source_count(&kg, f1.id).await, 1);

    // Explicit user edit of the same claim, overlapping → supersedes.
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2021, 1, 1)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    assert_ne!(f1.id, f2.id);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert_eq!(f1_after.status().unwrap(), FactStatus::Superseded);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
    // No source added to the superseded fact.
    assert_eq!(source_count(&kg, f1.id).await, 1);
    assert_eq!(source_count(&kg, f2.id).await, 1);
}

#[tokio::test]
async fn duplicate_source_is_noop() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    // Identical provenance — not an independent source, so a no-op.
    let result = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    assert_eq!(result.id, f1.id);
    assert_eq!(source_count(&kg, f1.id).await, 1);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((f1_after.confidence - 0.80).abs() < 1e-6);

    // The duplicate-provenance fast path is audit-silent: it must not emit a
    // SourceAdded or ConfidenceChange row for the unchanged fact.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::SourceAdded as i16).await,
        0
    );
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::ConfidenceChange as i16).await,
        0
    );
}

#[tokio::test]
async fn explicit_existing_corroborated_no_boost() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2020, 1, 1)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert!((f1.confidence - 1.0).abs() < 1e-6);
    assert_eq!(source_count(&kg, f1.id).await, 1);

    let result = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    assert_eq!(result.id, f1.id);
    // Source added for provenance, but confidence unchanged (explicit is capped).
    assert_eq!(source_count(&kg, f1.id).await, 2);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((f1_after.confidence - 1.0).abs() < 1e-6);
}

#[tokio::test]
async fn inferred_fact_corroboration_adds_source_no_boost() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let bob = create_person(&kg, "Bob").await;
    let london = create_place(&kg, "London").await;

    // Parent fact the inference is notionally derived from.
    let _parent = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2020, 1, 1)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Manually insert an inferred fact (confidence is structural, not boosted).
    let inferred = kg
        .insert_fact(NewFact {
            subject_id: bob,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2020, 1, 1)),
            valid_until: None,
            source_type: SourceType::Inference,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: Some(ExtractionMethod::InferenceRule),
            inferred: true,
            inference_depth: 1,
            confidence: Some(0.60),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert!(inferred.inferred);
    assert!((inferred.confidence - 0.60).abs() < 1e-6);
    assert_eq!(source_count(&kg, inferred.id).await, 1);

    let result = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                bob,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    assert_eq!(result.id, inferred.id);
    assert_eq!(source_count(&kg, inferred.id).await, 2);
    let after = kg.get_fact(inferred.id).await.unwrap().unwrap();
    // Inferred confidence is structural — corroboration must not boost it.
    assert!((after.confidence - 0.60).abs() < 1e-6);
}

#[tokio::test]
async fn corroboration_writes_audit_and_clears_stale_confidence() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    // Mark the fact's confidence as stale, as the nightly optimiser would.
    sqlx::query("UPDATE facts SET stale_confidence = TRUE WHERE id = ?")
        .bind(f1.id)
        .execute(kg.pool())
        .await
        .unwrap();

    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2021, 1, 1)),
            None,
            "s2",
            "r2",
        )
        .await,
    )
    .await
    .unwrap();

    // SourceAdded = 5, ConfidenceChange = 3.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::SourceAdded as i16).await,
        1
    );
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::ConfidenceChange as i16).await,
        1
    );

    let after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((after.confidence - 0.85).abs() < 1e-6);
    assert!(!after.stale_confidence);
}

#[tokio::test]
async fn corroboration_cascades_to_inferred_child() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    // Parent connector fact (connector default confidence 0.80).
    let parent = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert!((parent.confidence - 0.80).abs() < 1e-6);

    // Inferred child linked to the parent via an InferredFrom edge. Its
    // structural confidence for a single positive parent at depth 1 is
    // 0.80 * 0.8^1 * breadth(1)=0.6 = 0.384.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts \
         (subject_id, relationship_type_id, object_id, confidence, fact_status_id, \
          inferred, inference_depth, stale_confidence, pending_confirmation) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
    )
    .bind(alice)
    .bind(parent.relationship_type_id)
    .bind(london)
    .bind(0.384f32)
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
    .bind(parent.id)
    .bind(child.id)
    .bind(1i16) // InferredFrom
    .execute(kg.pool())
    .await
    .unwrap();

    // A corroborating connector fact boosts the parent 0.80 -> 0.85, which
    // must cascade and recalculate the child to 0.85 * 0.8 * 0.6 = 0.408.
    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2021, 1, 1)),
            None,
            "s2",
            "r2",
        )
        .await,
    )
    .await
    .unwrap();

    let parent_after = kg.get_fact(parent.id).await.unwrap().unwrap();
    assert!((parent_after.confidence - 0.85).abs() < 1e-6);

    let child_after = kg.get_fact(child.id).await.unwrap().unwrap();
    assert!(
        (child_after.confidence - 0.408).abs() < 1e-6,
        "expected child confidence 0.408, got {}",
        child_after.confidence
    );
    assert!(!child_after.stale_confidence);

    // The cascade writes a ConfidenceChange audit entry for the child.
    assert_eq!(
        audit_count(&kg, child.id, ChangeType::ConfidenceChange as i16).await,
        1
    );
}

#[tokio::test]
async fn explicit_system_overlapping_supersedes_not_corroborates() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(source_count(&kg, f1.id).await, 1);

    // An overlapping System assertion is explicit, so it supersedes rather
    // than corroborating (mirrors the UserEdit path).
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2021, 1, 1)),
            valid_until: None,
            source_type: SourceType::System,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    assert_ne!(f1.id, f2.id);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert_eq!(f1_after.status().unwrap(), FactStatus::Superseded);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
    // No source added to the superseded fact; the new fact owns one source.
    assert_eq!(source_count(&kg, f1.id).await, 1);
    assert_eq!(source_count(&kg, f2.id).await, 1);
}

// ---------------------------------------------------------------------------
// PR #174 review follow-ups: stale_confidence and diamond-graph cascade.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn corroboration_at_cap_clears_stale_confidence_without_audit() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert!((f1.confidence - 0.80).abs() < 1e-6);

    // Boost to the non-explicit cap: 0.80 -> 0.85 -> 0.90 -> 0.95.
    for (cid, rid) in [("s2", "r2"), ("s3", "r3"), ("s4", "r4")] {
        kg.insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                cid,
                rid,
            )
            .await,
        )
        .await
        .unwrap();
    }
    let at_cap = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((at_cap.confidence - 0.95).abs() < 1e-6);

    // Mark the capped fact stale, as the nightly optimiser would.
    sqlx::query("UPDATE facts SET stale_confidence = TRUE WHERE id = ?")
        .bind(f1.id)
        .execute(kg.pool())
        .await
        .unwrap();

    // A further independent corroboration cannot boost confidence (already at
    // the cap), but it adds provenance and must still clear the stale flag.
    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2022, 1, 1)),
            None,
            "s5",
            "r5",
        )
        .await,
    )
    .await
    .unwrap();

    let after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((after.confidence - 0.95).abs() < 1e-6);
    assert!(
        !after.stale_confidence,
        "corroboration at the cap must clear stale_confidence"
    );
    // Four corroborating sources added (s2..s5); the original source is not
    // audited as a SourceAdded event.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::SourceAdded as i16).await,
        4
    );
    // Only the three actual boosts wrote a ConfidenceChange; the capped
    // corroboration must not record a no-op confidence change.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::ConfidenceChange as i16).await,
        3
    );
}

#[tokio::test]
async fn corroboration_cascade_recalculates_through_diamond_graph() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    // Root connector fact A at the connector default 0.80.
    let a = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert!((a.confidence - 0.80).abs() < 1e-6);

    // Inferred children B and C of A (single positive parent, depth 1):
    // 0.80 * 0.8^1 * breadth(1)=0.6 = 0.384.
    let insert_inferred = |subject: i32, rel: i16, obj: i32, depth: i32, conf: f32| {
        let pool = kg.pool().clone();
        async move {
            let fact: mimir_knowledge::models::fact::Fact = sqlx::query_as(
                "INSERT INTO facts \
                 (subject_id, relationship_type_id, object_id, confidence, fact_status_id, \
                  inferred, inference_depth, stale_confidence, pending_confirmation) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id, subject_id, relationship_type_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
            )
            .bind(subject)
            .bind(rel)
            .bind(obj)
            .bind(conf)
            .bind(FactStatus::Inferred as i16)
            .bind(true)
            .bind(depth)
            .bind(false)
            .bind(false)
            .fetch_one(&pool)
            .await
            .unwrap();
            fact
        }
    };

    let b = insert_inferred(alice, a.relationship_type_id, london, 1, 0.384).await;
    let c = insert_inferred(alice, a.relationship_type_id, london, 1, 0.384).await;

    // Inferred child D of both B and C (two positive parents, depth 2):
    // (0.384 + 0.384) * 0.8^2 * breadth(2)=0.75 = 0.36864.
    let d = insert_inferred(alice, a.relationship_type_id, london, 2, 0.36864).await;

    for parent_id in [b.id, c.id] {
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id)
        .bind(d.id)
        .bind(1i16) // InferredFrom
        .execute(kg.pool())
        .await
        .unwrap();
    }
    for parent_id in [a.id] {
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id)
        .bind(b.id)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id)
        .bind(c.id)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();
    }

    // Corroborate A: 0.80 -> 0.85. The cascade must recalculate B and C to
    // 0.408 and then D from BOTH updated parents:
    // (0.408 + 0.408) * 0.64 * 0.75 = 0.39168.
    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2021, 1, 1)),
            None,
            "s2",
            "r2",
        )
        .await,
    )
    .await
    .unwrap();

    let b_after = kg.get_fact(b.id).await.unwrap().unwrap();
    let c_after = kg.get_fact(c.id).await.unwrap().unwrap();
    let d_after = kg.get_fact(d.id).await.unwrap().unwrap();
    assert!(
        (b_after.confidence - 0.408).abs() < 1e-6,
        "B: {}",
        b_after.confidence
    );
    assert!(
        (c_after.confidence - 0.408).abs() < 1e-6,
        "C: {}",
        c_after.confidence
    );
    assert!(
        (d_after.confidence - 0.39168).abs() < 1e-6,
        "D should reflect both updated parents, got {}",
        d_after.confidence
    );
    assert!(!d_after.stale_confidence);
}
