//! Integration tests for the entity management subsystem (#49).

use chrono::{TimeZone, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{AutoCompletePolicy, EventType, LocationType, RecurrenceType};
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
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
async fn test_alias_outranks_exact_name_when_bare_duplicate_exists() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Canonical entity with alias
    let canonical = kg
        .create_entity("Bob Smith", EntityType::Person, &["Bob"])
        .await
        .unwrap();

    // Bare-name duplicate (accidentally created before alias was wired)
    let duplicate = kg
        .create_entity("Bob", EntityType::Person, &[])
        .await
        .unwrap();

    // Searching for "Bob" should return the canonical entity first because
    // alias matches now outrank exact name matches.
    let results = mimir_knowledge::queries::entity::get_by_name(kg.pool(), "Bob")
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].entity.id, canonical.id);
    assert_eq!(results[0].match_kind, MatchKind::ExactAlias);

    // The duplicate should appear second (exact name match).
    assert_eq!(results[1].entity.id, duplicate.id);
    assert_eq!(results[1].match_kind, MatchKind::ExactName);
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
        kg.ensure_relationship_type("born_on").await.unwrap(),
        EntityType::DateTime,
    )
    .await
    .unwrap();

    // Valid: Organization located_in Place
    mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Organization,
        kg.ensure_relationship_type("located_in").await.unwrap(),
        EntityType::Place,
    )
    .await
    .unwrap();

    // Invalid: Place born_on Person (nonsense combination)
    let result = mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Place,
        kg.ensure_relationship_type("born_on").await.unwrap(),
        EntityType::Person,
    )
    .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Events & reminders (issue #74)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_event_overlay_surfaces_in_upcoming() {
    use mimir_knowledge::models::enums::{EventStatus, EventType};

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("David", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Tokyo".to_string()),
            valid_from: Some(now + chrono::Duration::days(5)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Scan derives a one-time overlay for the future-dated fact.
    let summary = kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.derived, 1);

    let event = kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(event.status(), Some(EventStatus::Active));
    assert_eq!(event.event_type(), Some(EventType::Reminder));
    assert!(!event.is_recurring());

    // The fact surfaces in the upcoming section.
    let section = mimir_knowledge::queries::memory::render_upcoming_section(
        kg.pool(),
        entity.id,
        now,
        30,
        10,
    )
    .await
    .unwrap();
    assert!(section.contains("Tokyo"), "section was: {section}");
}

#[tokio::test]
async fn test_event_auto_complete_on_date() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Eve", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Rome".to_string()),
            valid_from: Some(now - chrono::Duration::days(3)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    kg.insert_event(NewEvent {
        fact_id: fact.id,
        entity_id: entity.id,
        trigger_date: now - chrono::Duration::days(3),
        recurrence: RecurrenceType::None,
        event_type: EventType::Reminder,
        auto_complete_policy: AutoCompletePolicy::AutoCompleteOnDate,
        requires_user_action: false,
    })
    .await
    .unwrap();

    let summary = kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.completed, 1);

    let event = kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(event.status(), Some(EventStatus::Completed));
}

#[tokio::test]
async fn test_event_recurring_yearly_advances() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    // A birthday fact whose event trigger date is in the past.
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("birthday".to_string()),
            valid_from: Some(Utc.with_ymd_and_hms(1990, 5, 15, 0, 0, 0).unwrap()),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    let past_trigger = now - chrono::Duration::days(1);
    kg.insert_event(NewEvent {
        fact_id: fact.id,
        entity_id: entity.id,
        trigger_date: past_trigger,
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    let summary = kg.run_events_scan(365).await.unwrap();
    assert_eq!(summary.advanced, 1);

    let event = kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(event.status(), Some(EventStatus::Active));
    // Advanced trigger date is now in the future (next anniversary).
    assert!(event.trigger_date > now);
}

#[tokio::test]
async fn test_event_requires_user_action_becomes_overdue() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("post letter".to_string()),
            valid_from: Some(now - chrono::Duration::days(2)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    kg.insert_event(NewEvent {
        fact_id: fact.id,
        entity_id: entity.id,
        trigger_date: now - chrono::Duration::days(2),
        recurrence: RecurrenceType::None,
        event_type: EventType::Task,
        auto_complete_policy: AutoCompletePolicy::RequiresUserAction,
        requires_user_action: true,
    })
    .await
    .unwrap();

    // Scan must NOT auto-complete a RequiresUserAction event.
    let summary = kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.completed, 0);

    let overdue = kg.get_overdue_events(entity.id).await.unwrap();
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].status(), Some(EventStatus::Active));
}

#[tokio::test]
async fn test_event_scan_derive_is_idempotent() {
    use mimir_knowledge::models::enums::EventType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Idem", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Oslo".to_string()),
            valid_from: Some(now + chrono::Duration::days(5)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // First scan derives exactly one overlay.
    let first = kg.run_events_scan(30).await.unwrap();
    assert_eq!(first.derived, 1);

    // Pre-seed an overlay directly (simulates a concurrent writer) for a
    // second future fact, then run the scan: the existing overlay must not
    // cause a unique-key failure, and must not be counted again.
    let fact2 = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Lyon".to_string()),
            valid_from: Some(now + chrono::Duration::days(9)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    kg.insert_event(mimir_knowledge::models::event::NewEvent {
        fact_id: fact2.id,
        entity_id: entity.id,
        trigger_date: now + chrono::Duration::days(9),
        recurrence: RecurrenceType::None,
        event_type: EventType::Reminder,
        auto_complete_policy: AutoCompletePolicy::AutoCompleteOnDate,
        requires_user_action: false,
    })
    .await
    .unwrap();

    // Re-running the scan over the same fact derives nothing new (no error, no
    // duplicate), and the already-overlaid fact2 is also not re-derived.
    let second = kg.run_events_scan(30).await.unwrap();
    assert_eq!(second.derived, 0);
    assert_eq!(
        kg.get_event_by_fact(fact.id)
            .await
            .unwrap()
            .unwrap()
            .event_type_id,
        EventType::Reminder as i16
    );
}

#[tokio::test]
async fn test_recurring_requires_user_action_not_advanced() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Iris", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("file taxes".to_string()),
            valid_from: Some(now - chrono::Duration::days(1)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // A recurring deadline that also requires user action: the advance pass
    // must leave it untouched so it surfaces as overdue.
    let past_trigger = now - chrono::Duration::days(1);
    kg.insert_event(NewEvent {
        fact_id: fact.id,
        entity_id: entity.id,
        trigger_date: past_trigger,
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Task,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: true,
    })
    .await
    .unwrap();

    let summary = kg.run_events_scan(365).await.unwrap();
    assert_eq!(summary.advanced, 0);

    let event = kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(event.trigger_date, past_trigger);
    assert_eq!(event.status(), Some(EventStatus::Active));

    let overdue = kg.get_overdue_events(entity.id).await.unwrap();
    assert_eq!(overdue.len(), 1);
}

#[tokio::test]
async fn test_upcoming_and_scan_align_on_low_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Juno", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    // A low-confidence interaction fact with a future valid_from. The Upcoming
    // query gates on `confidence >= 0.5`; the derive scan must use the same
    // gate so it does not create a hidden overlay that never surfaces.
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Madrid".to_string()),
            valid_from: Some(now + chrono::Duration::days(6)),
            valid_until: None,
            source_type: SourceType::Interaction,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.3),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Scan must not derive an overlay for a sub-threshold fact.
    let summary = kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.derived, 0);
    assert!(kg.get_event_by_fact(fact.id).await.unwrap().is_none());

    // And the fact does not surface in Upcoming.
    let section = mimir_knowledge::queries::memory::render_upcoming_section(
        kg.pool(),
        entity.id,
        now,
        30,
        10,
    )
    .await
    .unwrap();
    assert!(!section.contains("Madrid"), "section was: {section}");
}

#[tokio::test]
async fn test_event_dismiss_excludes_from_upcoming() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Hank", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Berlin".to_string()),
            valid_from: Some(now + chrono::Duration::days(4)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    let event = kg
        .insert_event(NewEvent {
            fact_id: fact.id,
            entity_id: entity.id,
            trigger_date: now + chrono::Duration::days(4),
            recurrence: RecurrenceType::None,
            event_type: EventType::Reminder,
            auto_complete_policy: AutoCompletePolicy::AutoCompleteOnDate,
            requires_user_action: false,
        })
        .await
        .unwrap();

    kg.dismiss_event(event.id).await.unwrap();
    let after = kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(after.status(), Some(EventStatus::Dismissed));

    let section = mimir_knowledge::queries::memory::render_upcoming_section(
        kg.pool(),
        entity.id,
        now,
        30,
        10,
    )
    .await
    .unwrap();
    assert!(!section.contains("Berlin"), "section was: {section}");
}

#[tokio::test]
async fn test_recurrence_next_occurrence_leap_year() {
    use mimir_knowledge::models::recurrence::next_occurrence;

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

    // Insert a fact for x so x survives the merge (more facts = survivor)
    kg.insert_fact(NewFact {
        subject_id: x.id,
        relationship_type: "is_in".to_string(),
        object_id: None,
        object_literal: Some("somewhere".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_id: None,
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
        connector_id: None,
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
            connector_id: None,
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

    sqlx::query("INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)")
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
            connector_id: None,
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

#[tokio::test]
async fn test_recurring_event_not_duplicated_in_upcoming() {
    use mimir_knowledge::models::enums::EventStatus;
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Iris", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    let trigger = now + chrono::Duration::days(5);
    let fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Kyoto".to_string()),
            valid_from: Some(trigger),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // A recurring overlay on the same future-dated fact (trigger == valid_from).
    kg.insert_event(NewEvent {
        fact_id: fact.id,
        entity_id: entity.id,
        trigger_date: trigger,
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    let section = mimir_knowledge::queries::memory::render_upcoming_section(
        kg.pool(),
        entity.id,
        now,
        30,
        10,
    )
    .await
    .unwrap();
    let hits = section.matches("Kyoto").count();
    assert_eq!(hits, 1, "recurring event duplicated in section:\n{section}");

    // Overlay is active recurring (surfaced via the recurring query, not one-time).
    let event = kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(event.status(), Some(EventStatus::Active));
    assert!(event.is_recurring());
}
