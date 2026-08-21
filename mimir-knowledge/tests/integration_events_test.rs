//! Event overlay, recurrence, auto-complete, and upcoming-section integration tests.

use chrono::{TimeZone, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{AutoCompletePolicy, EventType, RecurrenceType};
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;

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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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
            connector_instance_id: None,
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

#[tokio::test]
async fn test_superseded_recurring_fact_overlay_retired() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;
    use mimir_knowledge::models::fact::FactStatus;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Nora", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    // A recurring anniversary fact with an active recurring overlay whose next
    // occurrence falls inside the upcoming horizon.
    let old_fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("15 February".to_string()),
            valid_from: Some(now - chrono::Duration::days(400)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
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
        fact_id: old_fact.id,
        entity_id: entity.id,
        trigger_date: now + chrono::Duration::days(5),
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    // Correct the anniversary: an explicit overlapping fact supersedes the old
    // one (issue #413).
    let new_fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("16 February".to_string()),
            valid_from: Some(now + chrono::Duration::days(6)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
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

    // The old fact is superseded and its overlay is retired (dismissed).
    let old_after = kg.get_fact(old_fact.id).await.unwrap().unwrap();
    assert_eq!(old_after.status().unwrap(), FactStatus::Superseded);
    let old_event = kg.get_event_by_fact(old_fact.id).await.unwrap().unwrap();
    assert_eq!(old_event.status(), Some(EventStatus::Dismissed));

    // The scan must not advance the retired overlay, and must derive a fresh
    // one for the corrected fact.
    let summary = kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.advanced, 0);
    assert_eq!(summary.derived, 1);
    let old_event_after = kg.get_event_by_fact(old_fact.id).await.unwrap().unwrap();
    assert_eq!(old_event_after.trigger_date, old_event.trigger_date);

    // Only the corrected date surfaces in the Upcoming section.
    let section = mimir_knowledge::queries::memory::render_upcoming_section(
        kg.pool(),
        entity.id,
        now,
        30,
        10,
    )
    .await
    .unwrap();
    assert!(section.contains("16 February"), "section was: {section}");
    assert!(!section.contains("15 February"), "section was: {section}");
    assert_ne!(old_fact.id, new_fact.id);
}

#[tokio::test]
async fn test_superseded_fact_overlay_not_advanced_or_surfaced() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventStatus, EventType};
    use mimir_knowledge::models::event::NewEvent;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Omar", EntityType::Person, &[])
        .await
        .unwrap();

    let now = Utc::now();
    // Two recurring facts with active overlays: one past-due (would advance on
    // scan) and one inside the upcoming horizon (would surface).
    let past_fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("past anniversary".to_string()),
            valid_from: Some(now - chrono::Duration::days(400)),
            // Bounded so it does not overlap (and get superseded by) the
            // second fact below — this test flips statuses directly.
            valid_until: Some(now - chrono::Duration::days(350)),
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
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
        fact_id: past_fact.id,
        entity_id: entity.id,
        trigger_date: now - chrono::Duration::days(1),
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    let future_fact = kg
        .insert_fact(NewFact {
            subject_id: entity.id,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("future anniversary".to_string()),
            valid_from: Some(now - chrono::Duration::days(300)),
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
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
        fact_id: future_fact.id,
        entity_id: entity.id,
        trigger_date: now + chrono::Duration::days(5),
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    // Simulate a supersession path that does not retire the overlay (e.g. a
    // legacy row or a future writer): flip the facts to Superseded directly.
    sqlx::query("UPDATE facts SET fact_status_id = ? WHERE id IN (?, ?)")
        .bind(mimir_knowledge::models::fact::FactStatus::Superseded as i16)
        .bind(past_fact.id)
        .bind(future_fact.id)
        .execute(kg.pool())
        .await
        .unwrap();

    // The scan must not advance overlays of superseded facts.
    let summary = kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.advanced, 0);
    let past_event = kg.get_event_by_fact(past_fact.id).await.unwrap().unwrap();
    assert_eq!(past_event.status(), Some(EventStatus::Active));
    assert_eq!(
        past_event.trigger_date,
        now - chrono::Duration::days(1),
        "superseded fact's overlay was advanced by the scan"
    );

    // The Upcoming section must not surface overlays of superseded facts.
    let section = mimir_knowledge::queries::memory::render_upcoming_section(
        kg.pool(),
        entity.id,
        now,
        30,
        10,
    )
    .await
    .unwrap();
    assert!(
        !section.contains("future anniversary"),
        "superseded fact surfaced in section: {section}"
    );
}
