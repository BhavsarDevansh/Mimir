//! Entity location round-trip and delete-guard integration tests.

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::LocationType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;

// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_location_stub_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Irene", EntityType::Person, &[])
        .await
        .unwrap();

    let loc = kg
        .insert_location(
            entity.id,
            LocationType::Home,
            Some("123 Maple St"),
            Some(40.7128),
            Some(-74.0060),
            Some("America/New_York"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(loc.entity_id, entity.id);
    assert_eq!(loc.address.as_deref(), Some("123 Maple St"));

    let locs = kg.get_locations(entity.id).await.unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].latitude, Some(40.7128));

    let updated = mimir_knowledge::queries::location::update_location(
        kg.pool(),
        loc.id,
        Some("456 Oak Ave"),
        None,
        None,
        Some("Europe/London"),
    )
    .await
    .unwrap();
    assert_eq!(updated.address.as_deref(), Some("456 Oak Ave"));
    assert_eq!(updated.timezone.as_deref(), Some("Europe/London"));
    assert_eq!(updated.latitude, Some(40.7128)); // unchanged
}

// ---------------------------------------------------------------------------
// Delete guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_guard_rejects_entity_with_facts() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jack", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jill", EntityType::Person, &[])
        .await
        .unwrap();
    let located_in_id = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .unwrap();

    sqlx::query("INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)")
        .bind(a.id)
        .bind(located_in_id)
        .bind(b.id)
        .bind(1.0f32)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();

    let result = kg.delete_entity(a.id).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("1 fact(s)"),
        "Expected fact count in error: {}",
        err
    );
}

#[tokio::test]
async fn test_delete_guard_rejects_entity_with_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jack", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert a fact for a to serve as source_fact_id
    let fact_a = kg
        .insert_fact(NewFact {
            subject_id: a.id,
            relationship_type: "has_preference".to_string(),
            object_id: None,
            object_literal: Some("pref".to_string()),
            valid_from: None,
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

    sqlx::query("INSERT INTO preferences (entity_id, category_id, key, value, confidence, source_fact_id) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(a.id)
        .bind(1i16)
        .bind("theme")
        .bind("dark")
        .bind(1.0f32)
        .bind(fact_a.id)
        .execute(kg.pool())
        .await
        .unwrap();

    let result = kg.delete_entity(a.id).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("2"),
        "Expected reference count in error: {}",
        err
    );
}

#[tokio::test]
async fn test_delete_guard_rejects_entity_in_merge_queue() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jack", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jill", EntityType::Person, &[])
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) VALUES (?, ?, ?)",
    )
    .bind(a.id)
    .bind(b.id)
    .bind(1i16)
    .execute(kg.pool())
    .await
    .unwrap();

    let result = kg.delete_entity(a.id).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("1"),
        "Expected reference count in error: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// LLM semantic dedup stub
