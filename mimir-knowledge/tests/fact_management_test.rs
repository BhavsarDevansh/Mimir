//! Integration tests for the fact management subsystem (#50).

use chrono::{TimeZone, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::Predicate;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;

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
        predicate: Predicate::IsIn,
        object_id: Some(london),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        confidence: None,
    };

    let fact = kg.insert_fact(new_fact.clone()).await.unwrap();
    assert_eq!(fact.subject_id, alice);
    assert_eq!(fact.predicate_id, Predicate::IsIn as i16);
    assert_eq!(fact.status(), FactStatus::Active);

    // Read back
    let fetched = kg.get_fact(fact.id).await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, fact.id);

    // Update status
    let updated = kg
        .update_fact_status(fact.id, FactStatus::Disputed)
        .await
        .unwrap();
    assert_eq!(updated.status(), FactStatus::Disputed);

    // Update valid_until
    let until = Utc.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap();
    let updated = kg
        .update_fact_valid_until(fact.id, Some(until))
        .await
        .unwrap();
    assert_eq!(updated.valid_until, Some(until));

    // Forget
    kg.forget_fact(fact.id, "test_user").await.unwrap();
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    assert_eq!(f1.status(), FactStatus::Active);
    assert_eq!(f2.status(), FactStatus::Active);
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    assert_eq!(f2.status(), FactStatus::Disputed);
}

// ---------------------------------------------------------------------------
// Temporal: open-ended old + new explicit → old gets closed, new Active
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(kg.now()),
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    assert_eq!(f2.status(), FactStatus::Active);

    let old = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!(old.valid_until.is_some());
    assert_eq!(old.status(), FactStatus::Active);
}

// ---------------------------------------------------------------------------
// Predicate id lookup roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_predicate_id_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let dev = create_person(&kg, "Developer").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::WorksAs,
            object_id: Some(dev),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    assert_eq!(fact.predicate_id, Predicate::WorksAs as i16);
    assert_eq!(fact.predicate(), Predicate::WorksAs);

    let by_predicate = kg
        .get_facts_by_predicate(Predicate::WorksAs, 10)
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    assert!(log.iter().any(|e| e.action == "INSERT"));

    kg.update_fact_status(fact.id, FactStatus::Disputed)
        .await
        .unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    assert!(log.iter().any(|e| e.action == "STATUS_CHANGE"));
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Calendar,
            confidence: None,
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    // Create an inferred child fact manually.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id, inferred) \
         VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at",
    )
    .bind(alice)
    .bind(Predicate::Visited as i16)
    .bind(london)
    .bind(0.5f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
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
    kg.forget_fact(parent.id, "test").await.unwrap();

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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    let parent_b = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::LocatedIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    // Inferred child with two parents.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id, inferred) \
         VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at",
    )
    .bind(alice)
    .bind(Predicate::Visited as i16)
    .bind(london)
    .bind(0.8f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
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
    kg.forget_fact(parent_a.id, "test").await.unwrap();

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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();

    kg.forget_fact(fact.id, "test").await.unwrap();

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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            confidence: None,
        })
        .await
        .unwrap();
    assert!((f_user.confidence - 1.0).abs() < f32::EPSILON);

    let f_inf = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::Visited,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::Inference,
            confidence: None,
        })
        .await
        .unwrap();
    assert!((f_inf.confidence - 0.50).abs() < f32::EPSILON);

    let f_conn = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::Owns,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::Connector,
            confidence: None,
        })
        .await
        .unwrap();
    assert!((f_conn.confidence - 0.80).abs() < f32::EPSILON);
}
