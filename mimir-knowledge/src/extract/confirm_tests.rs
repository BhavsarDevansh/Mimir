use super::*;

use crate::clock::MockClock;
use crate::extract::process_remember_output;
use crate::extract::schema::{Classification, ExtractedFact, RememberOutput, Temporal};
use chrono::{DateTime, Duration, Utc};

/// Fresh KnowledgeGraph with a controllable clock for time-sensitive tests.
async fn fresh_kg_with_clock(
    start: DateTime<Utc>,
) -> (KnowledgeGraph, std::sync::Arc<MockClock>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let clock = std::sync::Arc::new(MockClock::new(start));
    let kg = KnowledgeGraph::init_with_clock(&dir.path().join("confirm_test.db"), clock.clone())
        .await
        .unwrap();
    (kg, clock, dir)
}

fn sensitive_allergy_fact(object: &str) -> ExtractedFact {
    ExtractedFact {
        classification: Classification::Explicit,
        subject: "Devansh".to_string(),
        subject_type: "Person".to_string(),
        relationship_type: "allergy".to_string(),
        object: object.to_string(),
        object_is_entity: false,
        object_type: None,
        temporal: None,
        is_sensitive: true,
        correction_scope: None,
        categories: vec!["230".to_string()],
        recurrence: None,
        requires_user_action: None,
        location: None,
    }
}

async fn create_pending_fact(kg: &KnowledgeGraph, object: &str) -> i32 {
    let outcome = process_remember_output(
        kg,
        RememberOutput {
            facts: vec![sensitive_allergy_fact(object)],
        },
    )
    .await
    .expect("extraction should succeed");

    assert!(
        outcome.errors.is_empty(),
        "unexpected extraction errors: {:?}",
        outcome.errors
    );
    assert_eq!(outcome.pending_confirmation.len(), 1);
    outcome.pending_confirmation[0].fact_id
}

#[tokio::test]
async fn confirm_flips_status_to_active_and_confidence_to_one() {
    let (kg, _clock, _dir) = fresh_kg_with_clock(
        DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .into(),
    )
    .await;
    let fact_id = create_pending_fact(&kg, "peanuts").await;

    let fact = kg.get_fact(fact_id).await.unwrap().expect("fact exists");
    assert_eq!(fact.status(), Some(FactStatus::Disputed));
    assert!(fact.pending_confirmation);

    let confirmed = kg
        .confirm_fact(fact_id)
        .await
        .expect("confirm should succeed");

    assert_eq!(confirmed.status(), Some(FactStatus::Active));
    assert!((confirmed.confidence - 1.0).abs() < f32::EPSILON);
    assert!(!confirmed.pending_confirmation);

    // In-memory cache updated.
    assert!(!kg.pending_confirmations().read().await.contains(&fact_id));
}

#[tokio::test]
async fn confirm_rejects_non_pending_fact() {
    let (kg, _clock, _dir) = fresh_kg_with_clock(
        DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .into(),
    )
    .await;
    let fact_id = create_pending_fact(&kg, "peanuts").await;
    kg.confirm_fact(fact_id).await.unwrap();

    // Second confirm must fail: the fact is no longer pending.
    let err = kg.confirm_fact(fact_id).await.unwrap_err();
    assert!(matches!(err, KnowledgeError::Validation(_)));
}

/// Build a sensitive ExtractedFact with explicit event metadata so the
/// pending-confirmation overlay-rebuild path can be exercised.
fn sensitive_event_fact(
    object: &str,
    valid_from: Option<&str>,
    recurrence: Option<&str>,
    requires_user_action: Option<bool>,
) -> ExtractedFact {
    ExtractedFact {
        classification: Classification::Explicit,
        subject: "Devansh".to_string(),
        subject_type: "Person".to_string(),
        relationship_type: "allergy".to_string(),
        object: object.to_string(),
        object_is_entity: false,
        object_type: None,
        temporal: valid_from.map(|vf| Temporal {
            valid_from: Some(vf.to_string()),
            valid_until: None,
        }),
        is_sensitive: true,
        correction_scope: None,
        categories: vec!["230".to_string()],
        recurrence: recurrence.map(|r| r.to_string()),
        requires_user_action,
        location: None,
    }
}

/// Insert a sensitive fact with event metadata and return its pending id.
async fn create_pending_event_fact(
    kg: &KnowledgeGraph,
    object: &str,
    valid_from: Option<&str>,
    recurrence: Option<&str>,
    requires_user_action: Option<bool>,
) -> i32 {
    let outcome = process_remember_output(
        kg,
        RememberOutput {
            facts: vec![sensitive_event_fact(
                object,
                valid_from,
                recurrence,
                requires_user_action,
            )],
        },
    )
    .await
    .expect("extraction should succeed");
    assert!(
        outcome.errors.is_empty(),
        "unexpected errors: {:?}",
        outcome.errors
    );
    assert_eq!(outcome.pending_confirmation.len(), 1);
    outcome.pending_confirmation[0].fact_id
}

#[tokio::test]
async fn confirm_preserves_recurring_event_metadata() {
    // A sensitive yearly-recurring reminder must keep its recurrence and
    // `Recurring` policy across the confirmation boundary, instead of being
    // flattened to a one-time `Reminder` (PR #173).
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, _clock, _dir) = fresh_kg_with_clock(start).await;
    let fact_id = create_pending_event_fact(
        &kg,
        "penicillin",
        Some("2024-06-01T09:00:00Z"),
        Some("yearly"),
        None,
    )
    .await;

    kg.confirm_fact(fact_id).await.expect("confirm succeeds");

    let event = queries::event::get_by_fact(kg.pool(), fact_id)
        .await
        .unwrap()
        .expect("overlay created on confirm");
    assert_eq!(event.recurrence(), Some(RecurrenceType::Yearly));
    assert_eq!(event.event_type(), Some(EventType::Reminder));
    assert_eq!(event.policy(), Some(AutoCompletePolicy::Recurring));
    assert!(!event.requires_user_action);
    assert_eq!(
        event.trigger_date,
        DateTime::parse_from_rfc3339("2024-06-01T09:00:00Z")
            .unwrap()
            .with_timezone::<Utc>(&Utc)
    );

    // The consumed metadata must be cleaned up.
    assert!(
        queries::event::get_pending_event_meta(kg.pool(), fact_id)
            .await
            .unwrap()
            .is_none(),
        "pending_event_meta should be removed after confirm"
    );
}

#[tokio::test]
async fn confirm_preserves_user_action_event_metadata() {
    // A sensitive task/deadline must keep `requires_user_action` and the
    // `RequiresUserAction` policy across confirmation, surfacing as overdue
    // rather than auto-completing (PR #173).
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, _clock, _dir) = fresh_kg_with_clock(start).await;
    let fact_id = create_pending_event_fact(
        &kg,
        "file tax return",
        Some("2024-04-30T17:00:00Z"),
        None,
        Some(true),
    )
    .await;

    kg.confirm_fact(fact_id).await.expect("confirm succeeds");

    let event = queries::event::get_by_fact(kg.pool(), fact_id)
        .await
        .unwrap()
        .expect("overlay created on confirm");
    assert_eq!(event.recurrence(), Some(RecurrenceType::None));
    assert_eq!(event.event_type(), Some(EventType::Task));
    assert_eq!(event.policy(), Some(AutoCompletePolicy::RequiresUserAction));
    assert!(event.requires_user_action);
}

#[tokio::test]
async fn confirm_legacy_pending_fact_falls_back_to_one_time_reminder() {
    // A future-dated pending fact with no persisted event metadata (e.g.
    // created before the pending_event_meta table) still gets a one-time
    // Reminder overlay via the legacy `valid_from > now` fallback path.
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, _clock, _dir) = fresh_kg_with_clock(start).await;
    let fact_id = create_pending_event_fact(
        &kg,
        "penicillin",
        Some("2024-06-01T09:00:00Z"),
        Some("yearly"),
        Some(true),
    )
    .await;

    // Simulate a legacy pending fact by removing the persisted metadata,
    // so the extracted recurrence/user-action metadata is lost and confirm
    // must fall back to a one-time Reminder.
    sqlx::query("DELETE FROM pending_event_meta WHERE fact_id = ?")
        .bind(fact_id)
        .execute(kg.pool())
        .await
        .unwrap();

    kg.confirm_fact(fact_id).await.expect("confirm succeeds");

    let event = queries::event::get_by_fact(kg.pool(), fact_id)
        .await
        .unwrap()
        .expect("legacy fallback creates a one-time overlay");
    assert_eq!(event.recurrence(), Some(RecurrenceType::None));
    assert_eq!(event.event_type(), Some(EventType::Reminder));
    assert_eq!(event.policy(), Some(AutoCompletePolicy::AutoCompleteOnDate));
    assert!(!event.requires_user_action);
    assert_eq!(
        event.trigger_date,
        DateTime::parse_from_rfc3339("2024-06-01T09:00:00Z")
            .unwrap()
            .with_timezone::<Utc>(&Utc)
    );
}

#[tokio::test]
async fn confirm_legacy_pending_fact_without_future_date_creates_no_overlay() {
    // A legacy pending fact with no future `valid_from` and no persisted
    // metadata creates no overlay (the fallback only fires for future
    // dates).
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, _clock, _dir) = fresh_kg_with_clock(start).await;
    let fact_id = create_pending_fact(&kg, "peanuts").await;

    sqlx::query("DELETE FROM pending_event_meta WHERE fact_id = ?")
        .bind(fact_id)
        .execute(kg.pool())
        .await
        .unwrap();

    kg.confirm_fact(fact_id).await.expect("confirm succeeds");

    assert!(
        queries::event::get_by_fact(kg.pool(), fact_id)
            .await
            .unwrap()
            .is_none(),
        "non-future legacy fact should not get an overlay"
    );
}

#[tokio::test]
async fn reject_hard_deletes_fact_and_writes_audit() {
    let (kg, _clock, _dir) = fresh_kg_with_clock(
        DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .into(),
    )
    .await;
    let fact_id = create_pending_fact(&kg, "peanuts").await;

    kg.reject_fact(fact_id, None)
        .await
        .expect("reject should succeed");

    // Fact is gone.
    assert!(kg.get_fact(fact_id).await.unwrap().is_none());

    // Audit trail persists (foreign keys do not cascade on hard delete).
    let audit = kg.get_audit_log(fact_id).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.change_type_id == ChangeType::Rejected as i16),
        "expected a Rejected audit entry, got: {:?}",
        audit
    );

    // In-memory cache updated.
    assert!(!kg.pending_confirmations().read().await.contains(&fact_id));
}

#[tokio::test]
async fn reject_clears_dependency_edges_before_hard_delete() {
    // `fact_dependencies` uses ON DELETE RESTRICT (migration 017), so a
    // pending fact participating in a dependency edge can only be
    // hard-deleted once those rows are removed. Reject must clear them.
    let (kg, _clock, _dir) = fresh_kg_with_clock(
        DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .into(),
    )
    .await;
    let parent_id = create_pending_fact(&kg, "peanuts").await;
    let child_id = create_pending_fact(&kg, "shellfish").await;

    sqlx::query(
        "INSERT INTO fact_dependencies \
             (parent_fact_id, child_fact_id, relation_type_id, is_positive) \
             VALUES (?, ?, ?, TRUE)",
    )
    .bind(parent_id)
    .bind(child_id)
    .bind(crate::models::enums::RelationType::InferredFrom as i16)
    .execute(kg.pool())
    .await
    .expect("seed dependency edge");

    // Rejecting the parent must not trip the RESTRICT FK.
    kg.reject_fact(parent_id, None)
        .await
        .expect("reject should clear dependencies and delete the fact");

    assert!(kg.get_fact(parent_id).await.unwrap().is_none());
    let audit = kg.get_audit_log(parent_id).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.change_type_id == ChangeType::Rejected as i16),
        "expected a Rejected audit entry, got: {:?}",
        audit
    );

    // The dependency edge referencing the rejected fact is gone.
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fact_dependencies \
             WHERE parent_fact_id = ? OR child_fact_id = ?",
    )
    .bind(parent_id)
    .bind(parent_id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn list_pending_returns_only_pending_facts() {
    let (kg, _clock, _dir) = fresh_kg_with_clock(
        DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .into(),
    )
    .await;
    let pending_id = create_pending_fact(&kg, "peanuts").await;

    let rows = kg.list_pending_facts().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fact_id, pending_id);
    assert_eq!(rows[0].subject, "Devansh");
    assert_eq!(rows[0].predicate, "allergy");
    assert_eq!(rows[0].object.as_deref(), Some("peanuts"));

    // Confirming removes it from the pending list.
    kg.confirm_fact(pending_id).await.unwrap();
    let rows = kg.list_pending_facts().await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn cleanup_deletes_only_stale_pending_facts() {
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, clock, _dir) = fresh_kg_with_clock(start).await;

    // Insert a pending fact at the start time.
    let stale_id = create_pending_fact(&kg, "peanuts").await;

    // Advance the clock past the 7-day retention window and insert a fresh
    // pending fact (distinct object) that should survive cleanup.
    // pending fact that should survive cleanup.
    clock.advance(Duration::days(8));
    let fresh_id = create_pending_fact(&kg, "shellfish").await;

    let deleted = kg.delete_stale_pending(7).await.unwrap();
    assert_eq!(deleted, 1);

    assert!(kg.get_fact(stale_id).await.unwrap().is_none());
    assert!(kg.get_fact(fresh_id).await.unwrap().is_some());

    // Remaining pending list contains only the fresh fact.
    let rows = kg.list_pending_facts().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fact_id, fresh_id);
}
