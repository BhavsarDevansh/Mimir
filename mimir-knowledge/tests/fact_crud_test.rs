//! Fact CRUD, predicate lookup, source attachment, and enum mapping integration tests (#50).

mod common;
use chrono::{TimeZone, Utc};
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
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
    };

    let fact = kg.insert_fact(new_fact.clone()).await.unwrap();
    assert_eq!(fact.subject_id, alice);
    let located_in_id = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fact.relationship_type_id, located_in_id);
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
// Predicate id lookup roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fact_relationship_type_id_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let dev = kg
        .create_entity("Developer", EntityType::Activity, &[])
        .await
        .unwrap()
        .id;

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
            relationship_type: "located_in".to_string(),
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
    // located_in is the canonical containment verb (is_in is its alias) —
    // assert name instead of a hardcoded id.
    assert_eq!(
        kg.relationship_type_name(fetched.relationship_type_id)
            .await,
        Some("located_in".to_string())
    );

    // Assert that an uninserted predicate ID returns None.
    assert_eq!(kg.relationship_type_name(999i16).await, None);
}
