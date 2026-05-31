//! Integration tests for the entity management subsystem (#49).

use chrono::{TimeZone, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EntityDateType, LocationType, Predicate, RecurrenceType};
use mimir_knowledge::queries::entity::MatchKind;

// ---------------------------------------------------------------------------
// Entity CRUD roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_crud_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Create
    let entity = kg
        .create_entity("Alice", EntityType::Person, &["A. Smith", "Ally"])
        .await
        .unwrap();
    assert_eq!(entity.name, "Alice");
    assert_eq!(entity.entity_type_id, EntityType::Person as i16);

    // Read by ID
    let fetched = kg.get_entity(entity.id).await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.name, "Alice");

    // Update
    let updated = kg
        .update_entity(entity.id, "Alice Smith", EntityType::Person)
        .await
        .unwrap();
    assert_eq!(updated.name, "Alice Smith");

    // Delete (no facts attached)
    kg.delete_entity(entity.id).await.unwrap();
    let gone = kg.get_entity(entity.id).await.unwrap();
    assert!(gone.is_none());
}

// ---------------------------------------------------------------------------
// Alias resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_alias_resolution_exact() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Bob", EntityType::Person, &["Bobby", "Robert"])
        .await
        .unwrap();

    // Exact name match
    let results = mimir_knowledge::queries::entity::get_by_name(kg.pool(), "Bob")
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].match_kind, MatchKind::ExactName);
    assert_eq!(results[0].entity.id, entity.id);

    // Exact alias match
    let results = mimir_knowledge::queries::entity::get_by_name(kg.pool(), "Bobby")
        .await
        .unwrap();
    assert!(!results.is_empty());
    let alias_match = results
        .iter()
        .find(|r| r.match_kind == MatchKind::ExactAlias);
    assert!(alias_match.is_some());
    assert_eq!(alias_match.unwrap().entity.id, entity.id);
}

#[tokio::test]
async fn test_alias_resolution_fuzzy() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    kg.create_entity("Charlotte", EntityType::Person, &[])
        .await
        .unwrap();

    // FTS5 search for exact word (still routed through FTS5 → MatchKind::Fuzzy)
    let results = kg.search_entities("Charlotte", 10).await.unwrap();
    assert!(!results.is_empty());
    let fuzzy = results.iter().find(|r| r.match_kind == MatchKind::Fuzzy);
    assert!(fuzzy.is_some());
}

// ---------------------------------------------------------------------------
// Entity type enum sync (DateTime = 8)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_type_enum_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let variants: &[(i16, &str, EntityType)] = &[
        (1, "Person", EntityType::Person),
        (2, "Place", EntityType::Place),
        (3, "Event", EntityType::Event),
        (4, "Object", EntityType::Object),
        (5, "Concept", EntityType::Concept),
        (6, "Organization", EntityType::Organization),
        (7, "Activity", EntityType::Activity),
        (8, "DateTime", EntityType::DateTime),
    ];

    for (expected_id, expected_name, variant) in variants {
        let (db_id, db_name): (i16, String) =
            sqlx::query_as("SELECT id, name FROM entity_types WHERE id = ?")
                .bind(*variant as i16)
                .fetch_one(kg.pool())
                .await
                .unwrap();
        assert_eq!(db_id, *expected_id);
        assert_eq!(db_name, *expected_name);

        // Roundtrip
        let row: (i16,) = sqlx::query_as("SELECT id FROM entity_types WHERE id = ? LIMIT 1")
            .bind(*variant as i16)
            .fetch_one(kg.pool())
            .await
            .unwrap();
        assert_eq!(row.0, *expected_id);
    }

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM entity_types")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(count, variants.len() as i64);
}

// ---------------------------------------------------------------------------
// Predicate validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_predicate_validation() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Valid: Person born_on DateTime
    mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Person,
        Predicate::BornOn,
        EntityType::DateTime,
    )
    .await
    .unwrap();

    // Valid: Organization located_in Place
    mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Organization,
        Predicate::LocatedIn,
        EntityType::Place,
    )
    .await
    .unwrap();

    // Invalid: Place born_on Person (nonsense combination)
    let result = mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Place,
        Predicate::BornOn,
        EntityType::Person,
    )
    .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Entity dates & recurrence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_date_recurrence_yearly() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("David", EntityType::Person, &[])
        .await
        .unwrap();

    let date = kg
        .insert_entity_date(
            entity.id,
            EntityDateType::Birth,
            "1990-05-15",
            RecurrenceType::Yearly,
            None,
            1.0,
        )
        .await
        .unwrap();
    assert_eq!(date.entity_id, entity.id);
    assert_eq!(date.date_value, "1990-05-15");

    let dates = kg.get_entity_dates(entity.id).await.unwrap();
    assert_eq!(dates.len(), 1);

    // Upcoming dates within 365 days from 2024-01-01 should include 2024-05-15
    let upcoming = mimir_knowledge::queries::entity::get_upcoming_dates(
        kg.pool(),
        entity.id,
        365,
        Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(upcoming.len(), 1);
}

#[tokio::test]
async fn test_entity_date_recurrence_leap_year() {
    use mimir_knowledge::models::entity_date::next_occurrence;

    let from = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
    let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, from);
    let expected = Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap();
    assert_eq!(result, Some(expected));

    let from = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, from);
    let expected = Utc.with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
    assert_eq!(result, Some(expected));
}

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

    // Insert a fact referencing y so we can verify FK repointing.
    sqlx::query("INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)")
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
    let (subject_id,): (i32,) =
        sqlx::query_as("SELECT subject_id FROM facts WHERE predicate_id = 1")
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

    // Insert a date for y
    kg.insert_entity_date(
        y.id,
        EntityDateType::Birth,
        "2024-06-01",
        RecurrenceType::None,
        Some("birthday"),
        1.0,
    )
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
    )
    .await
    .unwrap();

    // Insert a preference for y (direct SQL since no helper yet)
    sqlx::query("INSERT INTO preferences (entity_id, category_id, key, value, confidence) VALUES (?, ?, ?, ?, ?)")
        .bind(y.id)
        .bind(1i16)
        .bind("theme")
        .bind("\"dark\"")
        .bind(1.0f32)
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

    // Date should now belong to x
    let dates = mimir_knowledge::queries::entity::get_dates_for_entity(kg.pool(), x.id)
        .await
        .unwrap();
    assert_eq!(dates.len(), 1);
    assert_eq!(dates[0].entity_id, x.id);

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
        )
        .await
        .unwrap();
    assert_eq!(loc.entity_id, entity.id);
    assert_eq!(loc.address.as_deref(), Some("123 Maple St"));

    let locs = kg.get_locations(entity.id).await.unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].latitude, Some(40.7128));

    let updated = mimir_knowledge::queries::entity::update_location(
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

    sqlx::query("INSERT INTO facts (subject_id, predicate_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)")
        .bind(a.id)
        .bind(1i16)
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

    sqlx::query("INSERT INTO preferences (entity_id, category_id, key, value, confidence) VALUES (?, ?, ?, ?, ?)")
        .bind(a.id)
        .bind(1i16)
        .bind("theme")
        .bind("\"dark\"")
        .bind(1.0f32)
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
