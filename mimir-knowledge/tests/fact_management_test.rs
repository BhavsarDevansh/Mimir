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
    };

    let fact = kg.insert_fact(new_fact.clone()).await.unwrap();
    assert_eq!(fact.subject_id, alice);
    assert_eq!(fact.predicate_id, Predicate::IsIn as i16);
    assert_eq!(fact.status().unwrap(), FactStatus::Active);

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
    assert_eq!(updated.status().unwrap(), FactStatus::Disputed);

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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
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
        })
        .await
        .unwrap();

    assert_eq!(fact.predicate_id, Predicate::WorksAs as i16);
    assert_eq!(fact.predicate().unwrap(), Predicate::WorksAs);

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
        })
        .await
        .unwrap();

    // Create an inferred child fact manually.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at",
    )
    .bind(alice)
    .bind(Predicate::Visited as i16)
    .bind(london)
    .bind(0.5f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(0i32)
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
        })
        .await
        .unwrap();

    // Inferred child with two parents.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at",
    )
    .bind(alice)
    .bind(Predicate::Visited as i16)
    .bind(london)
    .bind(0.8f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(0i32)
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
        })
        .await
        .unwrap();
    assert!((f_inf.confidence - 0.0).abs() < f32::EPSILON);

    let f_conn = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::Owns,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::Connector,
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
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
async fn unknown_predicate_id_returns_none() {
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
        })
        .await
        .unwrap();

    // Insert a dummy predicate ID that has no Rust enum mapping.
    sqlx::query("INSERT INTO predicates (id, name, description) VALUES (?, ?, ?)")
        .bind(999i16)
        .bind("UnknownPredicate")
        .bind("test")
        .execute(kg.pool())
        .await
        .unwrap();

    // Update the fact to reference the unknown predicate.
    sqlx::query("UPDATE facts SET predicate_id = ? WHERE id = ?")
        .bind(999i16)
        .bind(fact.id)
        .execute(kg.pool())
        .await
        .unwrap();

    let fetched = kg.get_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(fetched.predicate(), None);
    assert_eq!(fetched.predicate_id, 999);
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(boundary),
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // f2: [2021-01-01, 2022-01-01) — starts exactly at boundary
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(boundary),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    let active = kg
        .get_active_facts_at(alice, Predicate::IsIn, boundary)
        .await
        .unwrap();

    // Half-open semantics: f1 ends at boundary, so it is NOT active.
    // f2 starts at boundary, so it IS active.
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, f2.id);
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // New fact with explicit start → should close old_fact at now().
    let _new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(now),
            valid_until: None,
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    let log = kg.get_audit_log(old_fact.id).await.unwrap();
    let closure_entry = log.iter().find(|e| e.action == "UPDATE");
    assert!(
        closure_entry.is_some(),
        "Expected an UPDATE audit log entry for automatic closure"
    );
    let entry = closure_entry.unwrap();
    assert!(entry.old_value.is_some());
    assert!(entry.new_value.is_some());
    assert_eq!(entry.performer.as_deref(), Some("system"));
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(from),
            valid_until: Some(until),
            source_type: SourceType::UserEdit,
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
            predicate: Predicate::LocatedIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // Non-inferred child with confidence that will drop below 0.20 when parent is removed.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at",
    )
    .bind(alice)
    .bind(Predicate::Visited as i16)
    .bind(london)
    .bind(0.8f32)
    .bind(FactStatus::Active as i16)
    .bind(false)
    .bind(0i32)
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
    kg.forget_fact(parent.id, "test").await.unwrap();

    let child_alive = kg.get_fact(child.id).await.unwrap();
    assert!(child_alive.is_some());
    let child_alive = child_alive.unwrap();
    assert_eq!(child_alive.status().unwrap(), FactStatus::Disputed);

    let log = kg.get_audit_log(child.id).await.unwrap();
    let status_change = log.iter().find(|e| e.action == "STATUS_CHANGE");
    assert!(
        status_change.is_some(),
        "Expected a STATUS_CHANGE audit log entry for cascade Disputed"
    );
    let entry = status_change.unwrap();
    assert!(entry.old_value.is_some());
    assert!(entry.new_value.is_some());
    assert_eq!(entry.performer.as_deref(), Some("system"));
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // New explicit fact with temporal overlap.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
            valid_until: None,
            source_type: SourceType::UserEdit,
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
    let status_change = log.iter().find(|e| e.action == "STATUS_CHANGE");
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
        "INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id, inferred, inference_depth, stale_confidence) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id, subject_id, predicate_id, object_id, object_literal, valid_from, valid_until, confidence, fact_status_id, inferred, inference_depth, stale_confidence, created_at, updated_at",
    )
    .bind(alice)
    .bind(Predicate::IsIn as i16)
    .bind(london)
    .bind(0.5f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(0i32)
    .bind(false)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    // Explicit replacement.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::Email,
        })
        .await
        .unwrap();

    // Explicit replacement with temporal overlap.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap()),
            valid_until: None,
            source_type: SourceType::UserEdit,
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            valid_until: Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()),
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // New explicit fact with NON-overlapping range.
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: Some(Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap()),
            valid_until: None,
            source_type: SourceType::UserEdit,
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
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // Second explicit fact replaces first.
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
        })
        .await
        .unwrap();

    // Third explicit fact replaces second; first is already Superseded.
    let f3 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(berlin),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
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
