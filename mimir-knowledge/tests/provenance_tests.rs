//! Provenance and audit log tests (Issue #52).

use chrono::{TimeZone, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::{ChangeType, ChangedBy};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::Predicate;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::queries::audit::AuditLogFilter;

async fn create_person(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Person, &[])
        .await
        .unwrap()
        .id
}

async fn create_place(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Place, &[])
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn source_type_roundtrips_after_migration() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    for &st in &[
        SourceType::UserEdit,
        SourceType::Connector,
        SourceType::Inference,
        SourceType::Interaction,
        SourceType::Import,
        SourceType::System,
    ] {
        let (id,): (i16,) = sqlx::query_as("SELECT id FROM source_types WHERE id = ? LIMIT 1")
            .bind(st as i16)
            .fetch_one(kg.pool())
            .await
            .unwrap();
        assert_eq!(id, st as i16);
    }
}

#[tokio::test]
async fn sources_unique_constraint() {
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
            source_type: SourceType::Connector,
            connector_id: Some("gmail-1".to_string()),
            connector_type: None,
            raw_reference: Some("msg-123".to_string()),
            extraction_method: None,
        })
        .await
        .unwrap();

    let result = sqlx::query(
        "INSERT INTO sources (fact_id, source_type_id, connector_id, raw_reference, extracted_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(fact.id)
    .bind(SourceType::Connector as i16)
    .bind("gmail-1")
    .bind("msg-123")
    .bind(Utc::now())
    .execute(kg.pool())
    .await;

    assert!(result.is_err(), "Expected unique constraint violation");
}

#[tokio::test]
async fn audit_on_insert_creates_entry() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    assert_eq!(
        log.len(),
        1,
        "Expected exactly one audit entry after insert"
    );

    let entry = &log[0];
    assert_eq!(entry.change_type_id, ChangeType::Created as i16);
    assert_eq!(entry.changed_by_id, Some(ChangedBy::User as i16));
    assert!(entry.old_value.is_none());
    let new_value = entry.new_value.as_ref().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(new_value).unwrap();
    assert_eq!(parsed["fact_id"], fact.id);
    assert_eq!(parsed["confidence"], 1.0);
}

#[tokio::test]
async fn audit_on_temporal_update() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    let until = Utc.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap();
    kg.update_fact_valid_until(fact.id, Some(until), ChangedBy::User)
        .await
        .unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    let temporal = log
        .iter()
        .find(|e| e.change_type_id == ChangeType::TemporalUpdate as i16)
        .expect("Expected temporal_update audit entry");

    let old: serde_json::Value =
        serde_json::from_str(temporal.old_value.as_ref().unwrap()).unwrap();
    let new: serde_json::Value =
        serde_json::from_str(temporal.new_value.as_ref().unwrap()).unwrap();
    assert!(old.get("valid_until").is_some());
    assert!(old.get("fact_status_id").is_none());
    assert!(new.get("valid_until").is_some());
    assert!(new.get("fact_status_id").is_none());
}

#[tokio::test]
async fn audit_on_status_change() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    kg.update_fact_status(fact.id, FactStatus::Disputed, ChangedBy::User)
        .await
        .unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    let status = log
        .iter()
        .find(|e| e.change_type_id == ChangeType::StatusChange as i16)
        .expect("Expected status_change audit entry");

    let old: serde_json::Value = serde_json::from_str(status.old_value.as_ref().unwrap()).unwrap();
    let new: serde_json::Value = serde_json::from_str(status.new_value.as_ref().unwrap()).unwrap();
    assert!(old.get("fact_status_id").is_some());
    assert!(old.get("valid_until").is_none());
    assert!(new.get("fact_status_id").is_some());
    assert!(new.get("valid_until").is_none());
}

#[tokio::test]
async fn audit_on_forget() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();

    let log = kg.get_audit_log(fact.id).await.unwrap();
    let forgotten = log
        .iter()
        .find(|e| e.change_type_id == ChangeType::Forgotten as i16)
        .expect("Expected forgotten audit entry");

    assert!(forgotten.old_value.is_some());
    let snapshot: serde_json::Value =
        serde_json::from_str(forgotten.old_value.as_ref().unwrap()).unwrap();
    assert_eq!(snapshot["id"], fact.id);
    assert_eq!(snapshot["subject_id"], alice);
}

#[tokio::test]
async fn audit_on_confidence_cascade() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let parent_a = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    let parent_b = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(paris),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    let child = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::Visited,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Inference,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    for parent in [&parent_a, &parent_b] {
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id, is_positive) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(parent.id)
        .bind(child.id)
        .bind(1i16)
        .bind(true)
        .execute(kg.pool())
        .await
        .unwrap();
    }

    sqlx::query("UPDATE facts SET inferred = ?, fact_status_id = ? WHERE id = ?")
        .bind(true)
        .bind(FactStatus::Inferred as i16)
        .bind(child.id)
        .execute(kg.pool())
        .await
        .unwrap();

    let conf_two = mimir_knowledge::confidence::recalculate(kg.pool(), child.id)
        .await
        .unwrap();
    sqlx::query("UPDATE facts SET confidence = ? WHERE id = ?")
        .bind(conf_two)
        .bind(child.id)
        .execute(kg.pool())
        .await
        .unwrap();

    kg.forget_fact(parent_a.id, ChangedBy::User).await.unwrap();

    let log = kg.get_audit_log(child.id).await.unwrap();
    let cc = log
        .iter()
        .find(|e| e.change_type_id == ChangeType::ConfidenceChange as i16)
        .expect("Expected confidence_change audit entry after cascade");

    let old: serde_json::Value = serde_json::from_str(cc.old_value.as_ref().unwrap()).unwrap();
    let new: serde_json::Value = serde_json::from_str(cc.new_value.as_ref().unwrap()).unwrap();
    assert!(old.get("confidence").is_some());
    assert!(new.get("confidence").is_some());
    let old_conf = old["confidence"].as_f64().unwrap() as f32;
    let new_conf = new["confidence"].as_f64().unwrap() as f32;
    assert!(
        new_conf < old_conf,
        "confidence should drop after losing a parent"
    );
}

#[tokio::test]
async fn source_crud_adds_source_and_audit() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    let source = kg
        .add_source_to_fact(mimir_knowledge::queries::source::AddSourceRequest {
            fact_id: fact.id,
            source_type: SourceType::Connector,
            connector_id: Some("gmail-1".to_string()),
            connector_type: None,
            raw_reference: Some("msg-456".to_string()),
            extraction_method: Some(ExtractionMethod::StructuredParse),
            changed_by: ChangedBy::User,
        })
        .await
        .unwrap();

    assert_eq!(source.source_type_id, SourceType::Connector as i16);
    assert_eq!(source.connector_id.as_deref(), Some("gmail-1"));

    let sources = kg.get_sources_for_fact(fact.id).await.unwrap();
    assert_eq!(sources.len(), 2);

    let log = kg.get_audit_log(fact.id).await.unwrap();
    let added = log
        .iter()
        .find(|e| e.change_type_id == ChangeType::SourceAdded as i16)
        .expect("Expected source_added audit entry");
    assert!(added.old_value.is_none());
    let new_value: serde_json::Value =
        serde_json::from_str(added.new_value.as_ref().unwrap()).unwrap();
    assert_eq!(new_value["source_type_id"], SourceType::Connector as i16);
}

#[tokio::test]
async fn query_audit_log_filtered() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    kg.update_fact_status(fact.id, FactStatus::Disputed, ChangedBy::User)
        .await
        .unwrap();

    let filter = AuditLogFilter {
        entity_name: Some("Alice".to_string()),
        predicate_name: Some("is_in".to_string()),
        from: None,
        to: None,
        change_type: Some(ChangeType::StatusChange),
        limit: None,
        offset: None,
    };

    let rows = kg.query_audit_log(filter).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entity_name.as_deref(), Some("Alice"));
    assert_eq!(rows[0].change_type_name, "status_change");
    assert_eq!(rows[0].changed_by_name.as_deref(), Some("user"));
}

#[tokio::test]
async fn query_audit_log_includes_forgotten_facts() {
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
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();

    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();

    let filter = AuditLogFilter {
        entity_name: None,
        predicate_name: None,
        from: None,
        to: None,
        change_type: Some(ChangeType::Forgotten),
        limit: None,
        offset: None,
    };

    let rows = kg.query_audit_log(filter).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fact_id, fact.id);
    assert_eq!(rows[0].change_type_name, "forgotten");
    // Names are NULL because the fact was hard-deleted.
    assert!(rows[0].entity_name.is_none());
    assert!(rows[0].predicate_name.is_none());
}
