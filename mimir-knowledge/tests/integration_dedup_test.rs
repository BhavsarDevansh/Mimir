//! Deduplication merge, alias flagging, and dedup-stub integration tests.

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{AutoCompletePolicy, EventType, LocationType, RecurrenceType};
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;

// ---------------------------------------------------------------------------
// Dedup: exact merge + overlapping alias flagging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dedup_exact_merge() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Create two entities with the same name (case-insensitive)
    let a = kg
        .create_entity("Eve", EntityType::Person, &["Evelyn"])
        .await
        .unwrap();
    let b = kg
        .create_entity("eve", EntityType::Person, &["Evie"])
        .await
        .unwrap();

    // Since create_entity dedups on exact name, b should be the same as a
    assert_eq!(a.id, b.id);

    // To test actual merge, insert two distinct rows manually then merge.
    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert a fact for x so x survives the merge (more facts = survivor)
    kg.insert_fact(NewFact {
        subject_id: x.id,
        relationship_type: "is_in".to_string(),
        object_id: None,
        object_literal: Some("somewhere".to_string()),
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

    // Insert a fact referencing y so we can verify FK repointing.
    sqlx::query("INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)")
        .bind(y.id)
        .bind(1i16)
        .bind(x.id)
        .bind(1.0f32)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();

    // Merge y into x
    mimir_knowledge::queries::entity::auto_merge_pair(kg.pool(), x.id, y.id)
        .await
        .unwrap();

    // y should be gone
    let gone = kg.get_entity(y.id).await.unwrap();
    assert!(gone.is_none());

    // Fact should now point to x as subject
    let (subject_id,): (i32,) = sqlx::query_as(
        "SELECT subject_id FROM facts WHERE relationship_type_id = 1 AND object_id = ?",
    )
    .bind(x.id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(subject_id, x.id);
}

#[tokio::test]
async fn test_auto_merge_migrates_dates_locations_and_cleans_preferences_queue() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert a fact for x so x survives the merge (more facts = survivor)
    kg.insert_fact(NewFact {
        subject_id: x.id,
        relationship_type: "is_in".to_string(),
        object_id: None,
        object_literal: Some("somewhere".to_string()),
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

    // Insert a location for y
    kg.insert_location(
        y.id,
        LocationType::Home,
        Some("456 Oak Ave"),
        Some(40.0),
        Some(-74.0),
        Some("America/New_York"),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Insert a fact for y to serve as source_fact_id
    let fact_y = kg
        .insert_fact(NewFact {
            subject_id: y.id,
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

    // Insert an event overlay on fact_y to verify overlays migrate on merge.
    kg.insert_event(mimir_knowledge::models::event::NewEvent {
        fact_id: fact_y.id,
        entity_id: y.id,
        trigger_date: chrono::Utc::now() + chrono::Duration::days(7),
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    // Insert a preference for y (direct SQL since no helper yet)
    sqlx::query("INSERT INTO preferences (entity_id, category_id, key, value, confidence, source_fact_id) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(y.id)
        .bind(1i16)
        .bind("theme")
        .bind("dark")
        .bind(1.0f32)
        .bind(fact_y.id)
        .execute(kg.pool())
        .await
        .unwrap();

    // Insert a merge-queue entry referencing y as duplicate
    sqlx::query(
        "INSERT INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) VALUES (?, ?, ?)",
    )
    .bind(x.id)
    .bind(y.id)
    .bind(1i16)
    .execute(kg.pool())
    .await
    .unwrap();

    // Merge y into x
    mimir_knowledge::queries::entity::auto_merge_pair(kg.pool(), x.id, y.id)
        .await
        .unwrap();

    // y should be gone
    let gone = kg.get_entity(y.id).await.unwrap();
    assert!(gone.is_none());

    // Event overlay should now belong to x (entity_id repointed to survivor).
    let event = kg.get_event_by_fact(fact_y.id).await.unwrap().unwrap();
    assert_eq!(event.entity_id, x.id);

    // Location should now belong to x
    let locs = kg.get_locations(x.id).await.unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].entity_id, x.id);

    // Preference for y should be removed
    let pref_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM preferences WHERE entity_id = ?")
        .bind(y.id)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(pref_count.0, 0);

    // Merge-queue entry referencing y should be removed
    let queue_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM entity_merge_queue WHERE primary_entity_id = ? OR duplicate_entity_id = ?"
    )
    .bind(y.id)
    .bind(y.id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(queue_count.0, 0);
}

#[tokio::test]
async fn test_dedup_overlapping_alias_flag() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Hank", EntityType::Person, &["Henry"])
        .await
        .unwrap();
    let b = kg
        .create_entity("Henrietta", EntityType::Person, &["Henry"])
        .await
        .unwrap();

    // Flag overlapping aliases
    mimir_knowledge::queries::entity::flag_overlapping_aliases(kg.pool())
        .await
        .unwrap();

    let queue: Vec<(i32, i32, i16)> = sqlx::query_as(
        "SELECT primary_entity_id, duplicate_entity_id, status_id FROM entity_merge_queue",
    )
    .fetch_all(kg.pool())
    .await
    .unwrap();

    assert!(!queue.is_empty());
    let entry = queue
        .iter()
        .find(|e| (e.0 == a.id && e.1 == b.id) || (e.0 == b.id && e.1 == a.id));
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().2, 1); // Pending
}

// ---------------------------------------------------------------------------
// Entity location stubs

// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_find_exact_duplicates_returns_empty_when_no_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // create_entity now enforces case-insensitive uniqueness at the DB level,
    // so "alice" resolves to the existing "Alice" record.
    let a = kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("alice", EntityType::Person, &[])
        .await
        .unwrap();
    assert_eq!(a.id, b.id);

    let dups = mimir_knowledge::queries::entity::find_exact_duplicates(kg.pool())
        .await
        .unwrap();
    assert!(
        dups.is_empty(),
        "Expected no duplicates with DB-level uniqueness"
    );
}

#[tokio::test]
async fn test_semantic_dedup_stub_returns_not_yet_implemented() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let result = mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![]).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Not yet implemented")
    );
}
