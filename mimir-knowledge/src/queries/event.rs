//! Event overlay CRUD, lifecycle queries, and scan-job helpers (issue #74).

use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::KnowledgeError;
use crate::models::enums::{AutoCompletePolicy, EventStatus, RecurrenceType};
use crate::models::event::{Event, NewEvent};

/// A future-dated fact that has no event overlay yet (scan-job input).
#[derive(Debug, Clone, FromRow)]
pub struct FutureFact {
    pub fact_id: i32,
    pub entity_id: i32,
    pub valid_from: DateTime<Utc>,
}

/// Insert a new event overlay.
pub async fn insert_event(pool: &SqlitePool, new: &NewEvent) -> Result<Event, KnowledgeError> {
    let record = sqlx::query_as::<_, Event>(
        "INSERT INTO events \
         (fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, status_id, \
          auto_complete_policy_id, requires_user_action) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                   status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at",
    )
    .bind(new.fact_id)
    .bind(new.entity_id)
    .bind(new.trigger_date)
    .bind(new.recurrence as i16)
    .bind(new.event_type as i16)
    .bind(EventStatus::Active as i16)
    .bind(new.auto_complete_policy as i16)
    .bind(new.requires_user_action)
    .fetch_one(pool)
    .await?;
    Ok(record)
}

/// Insert an event overlay only when no overlay yet exists for `fact_id`.
///
/// Returns `Some(Event)` when a new row was inserted, or `None` when an overlay
/// for this `fact_id` already existed. The `events.fact_id` UNIQUE constraint
/// plus `ON CONFLICT DO NOTHING` makes this safe against concurrent writers
/// (e.g. a derive scan and an extraction running at once), so derivation is
/// idempotent.
pub async fn insert_event_if_absent(
    pool: &SqlitePool,
    new: &NewEvent,
) -> Result<Option<Event>, KnowledgeError> {
    let row = sqlx::query_as::<_, Event>(
        "INSERT INTO events          (fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, status_id,           auto_complete_policy_id, requires_user_action)          VALUES (?, ?, ?, ?, ?, ?, ?, ?)          ON CONFLICT(fact_id) DO NOTHING          RETURNING id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id,                    status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at",
    )
    .bind(new.fact_id)
    .bind(new.entity_id)
    .bind(new.trigger_date)
    .bind(new.recurrence as i16)
    .bind(new.event_type as i16)
    .bind(EventStatus::Active as i16)
    .bind(new.auto_complete_policy as i16)
    .bind(new.requires_user_action)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Fetch an event overlay by its underlying fact id.
pub async fn get_by_fact(pool: &SqlitePool, fact_id: i32) -> Result<Option<Event>, KnowledgeError> {
    let row = sqlx::query_as::<_, Event>(
        "SELECT id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at \
         FROM events WHERE fact_id = ?",
    )
    .bind(fact_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Every event overlay attached to one entity, regardless of status or date.
///
/// Backs the Obsidian export's `Dates` section (issue #62), which renders the
/// full lifecycle picture of an entity's time-bound facts — past, active, and
/// retired overlays alike.
pub async fn get_events_by_entity(
    pool: &SqlitePool,
    entity_id: i32,
) -> Result<Vec<Event>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Event>(
        "SELECT id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at \
         FROM events \
         WHERE entity_id = ? \
         ORDER BY trigger_date",
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Transition an event to a new lifecycle status, recording `addressed_at`
/// when the status is terminal (Completed/Dismissed).
pub async fn update_status(
    pool: &SqlitePool,
    event_id: i32,
    status: EventStatus,
    now: DateTime<Utc>,
) -> Result<Event, KnowledgeError> {
    let addressed_at = match status {
        EventStatus::Completed | EventStatus::Dismissed => Some(now),
        _ => None,
    };
    let row = sqlx::query_as::<_, Event>(
        "UPDATE events SET status_id = ?, addressed_at = COALESCE(?, addressed_at) \
         WHERE id = ? \
         RETURNING id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                   status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at",
    )
    .bind(status as i16)
    .bind(addressed_at)
    .bind(event_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Soft-delete an event by marking it `Dismissed`.
pub async fn soft_delete(
    pool: &SqlitePool,
    event_id: i32,
    now: DateTime<Utc>,
) -> Result<Event, KnowledgeError> {
    update_status(pool, event_id, EventStatus::Dismissed, now).await
}

/// Advance a recurring event's `trigger_date` to its next occurrence.
pub async fn advance_recurring_trigger(
    pool: &SqlitePool,
    event_id: i32,
    next_trigger: DateTime<Utc>,
) -> Result<Event, KnowledgeError> {
    let row = sqlx::query_as::<_, Event>(
        "UPDATE events SET trigger_date = ? WHERE id = ? \
         RETURNING id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                   status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at",
    )
    .bind(next_trigger)
    .bind(event_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Active events for an entity whose `trigger_date` falls within `[from, to]`.
pub async fn get_active_events(
    pool: &SqlitePool,
    entity_id: i32,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Event>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Event>(
        "SELECT id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at \
         FROM events \
         WHERE entity_id = ? AND status_id = ? AND trigger_date >= ? AND trigger_date <= ? \
         ORDER BY trigger_date",
    )
    .bind(entity_id)
    .bind(EventStatus::Active as i16)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Active events for an entity that are past their `trigger_date` (overdue).
pub async fn get_overdue_events(
    pool: &SqlitePool,
    entity_id: i32,
    now: DateTime<Utc>,
) -> Result<Vec<Event>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Event>(
        "SELECT id, fact_id, entity_id, trigger_date, recurrence_type_id, event_type_id, \
                status_id, auto_complete_policy_id, requires_user_action, addressed_at, created_at \
         FROM events \
         WHERE entity_id = ? AND status_id = ? AND trigger_date < ? \
         ORDER BY trigger_date",
    )
    .bind(entity_id)
    .bind(EventStatus::Active as i16)
    .bind(now)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Active recurring events eligible for trigger advancement.
///
/// Only `Recurring`-policy events that do not require user action and whose
/// `trigger_date` has already passed (`trigger_date < now`) are returned, so
/// the scan only loads and sorts rows that can actually advance.
/// `RequiresUserAction` recurring deadlines/tasks stay past their trigger date
/// and surface as overdue instead of being silently advanced. Overlays of
/// superseded/forgotten facts are excluded (issue #413) so a stale overlay can
/// never advance, even if a supersession path forgot to retire it.
pub async fn get_active_recurring(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Vec<Event>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Event>(
        "SELECT e.id, e.fact_id, e.entity_id, e.trigger_date, e.recurrence_type_id, \
                e.event_type_id, e.status_id, e.auto_complete_policy_id, \
                e.requires_user_action, e.addressed_at, e.created_at \
         FROM events e \
         JOIN facts f ON f.id = e.fact_id \
         WHERE e.status_id = ? \
           AND e.recurrence_type_id != ? \
           AND e.auto_complete_policy_id = ? \
           AND e.requires_user_action = 0 \
           AND e.trigger_date < ? \
           AND f.fact_status_id NOT IN (?, ?) \
         ORDER BY e.trigger_date",
    )
    .bind(EventStatus::Active as i16)
    .bind(RecurrenceType::None as i16)
    .bind(AutoCompletePolicy::Recurring as i16)
    .bind(now)
    .bind(crate::models::fact::FactStatus::Superseded as i16)
    .bind(crate::models::fact::FactStatus::Forgotten as i16)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Active one-time events with `AutoCompleteOnDate` whose `trigger_date` has
/// passed and should transition to `Completed`. Overlays of superseded/forgotten
/// facts are excluded (issue #413) so a stale overlay is never auto-completed.
pub async fn get_past_due_auto_complete(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Vec<Event>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Event>(
        "SELECT e.id, e.fact_id, e.entity_id, e.trigger_date, e.recurrence_type_id, \
                e.event_type_id, e.status_id, e.auto_complete_policy_id, \
                e.requires_user_action, e.addressed_at, e.created_at \
         FROM events e \
         JOIN facts f ON f.id = e.fact_id \
         WHERE e.status_id = ? AND e.auto_complete_policy_id = ? AND e.recurrence_type_id = ? \
           AND e.trigger_date < ? \
           AND f.fact_status_id NOT IN (?, ?) \
         ORDER BY e.trigger_date",
    )
    .bind(EventStatus::Active as i16)
    .bind(AutoCompletePolicy::AutoCompleteOnDate as i16)
    .bind(RecurrenceType::None as i16)
    .bind(now)
    .bind(crate::models::fact::FactStatus::Superseded as i16)
    .bind(crate::models::fact::FactStatus::Forgotten as i16)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Future-dated facts (within `horizon`) that do not yet have an event overlay.
///
/// These are candidates for the scan job's derive pass. Only non-superseded,
/// non-forgotten, confirmed facts with a future `valid_from` are considered.
/// The `confidence >= 0.5` gate mirrors the Upcoming-section query so the scan
/// only derives overlays for facts that will actually surface (no hidden
/// overlays for low-confidence interaction facts).
pub async fn get_future_facts_without_overlay(
    pool: &SqlitePool,
    now: DateTime<Utc>,
    horizon: DateTime<Utc>,
) -> Result<Vec<FutureFact>, KnowledgeError> {
    let rows = sqlx::query_as::<_, FutureFact>(
        "SELECT f.id AS fact_id, f.subject_id AS entity_id, f.valid_from \
         FROM facts f \
         LEFT JOIN events e ON e.fact_id = f.id \
         WHERE e.id IS NULL \
           AND f.valid_from IS NOT NULL \
           AND f.valid_from > ? \
           AND f.valid_from <= ? \
           AND f.fact_status_id NOT IN (?, ?) \
           AND f.pending_confirmation = 0 \
           AND f.confidence >= 0.5",
    )
    .bind(now)
    .bind(horizon)
    .bind(crate::models::fact::FactStatus::Superseded as i16)
    .bind(crate::models::fact::FactStatus::Forgotten as i16)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Persisted event shape for a pending sensitive fact, used to rebuild the
/// overlay on confirmation instead of synthesising one-time defaults.
///
/// Only the shape fields are stored: `trigger_date` comes from the confirmed
/// fact's `valid_from`, and `fact_id`/`entity_id` from the confirmed fact. The
/// row is consumed by [`delete_pending_event_meta`] once the overlay is rebuilt.
#[derive(Debug, Clone, Copy, FromRow)]
pub struct PendingEventMeta {
    pub recurrence_type_id: i16,
    pub event_type_id: i16,
    pub auto_complete_policy_id: i16,
    pub requires_user_action: bool,
}

/// Persist the derived event shape for a pending sensitive fact.
///
/// Idempotent (`ON CONFLICT DO UPDATE`) so re-extraction of the same pending
/// fact refreshes the shape rather than failing.
pub async fn insert_pending_event_meta(
    pool: &SqlitePool,
    fact_id: i32,
    new: &NewEvent,
) -> Result<(), KnowledgeError> {
    sqlx::query(
        "INSERT INTO pending_event_meta \
         (fact_id, recurrence_type_id, event_type_id, auto_complete_policy_id, requires_user_action) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(fact_id) DO UPDATE SET \
          recurrence_type_id = excluded.recurrence_type_id, \
          event_type_id = excluded.event_type_id, \
          auto_complete_policy_id = excluded.auto_complete_policy_id, \
          requires_user_action = excluded.requires_user_action",
    )
    .bind(fact_id)
    .bind(new.recurrence as i16)
    .bind(new.event_type as i16)
    .bind(new.auto_complete_policy as i16)
    .bind(new.requires_user_action)
    .execute(pool)
    .await?;
    Ok(())
}

/// Read the persisted event shape for a pending sensitive fact, if any.
///
/// Returns `None` for legacy pending facts that predate the
/// `pending_event_meta` table; callers fall back to a one-time `Reminder`.
pub async fn get_pending_event_meta(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Option<PendingEventMeta>, KnowledgeError> {
    let row = sqlx::query_as::<_, PendingEventMeta>(
        "SELECT recurrence_type_id, event_type_id, auto_complete_policy_id, requires_user_action \
         FROM pending_event_meta WHERE fact_id = ?",
    )
    .bind(fact_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Remove the persisted event shape once the overlay has been rebuilt on
/// confirmation. Rejecting a fact hard-deletes the row via `ON DELETE CASCADE`,
/// so this is only needed for the confirm path.
pub async fn delete_pending_event_meta(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<(), KnowledgeError> {
    sqlx::query("DELETE FROM pending_event_meta WHERE fact_id = ?")
        .bind(fact_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Retire the event overlay of a fact that is no longer valid (issue #413).
///
/// Dismisses any non-terminal overlay (`Pending`/`Active`/`Snoozed`) so it
/// stops advancing and surfacing, and drops any persisted pending-event shape.
/// `Completed` overlays are preserved: they are historical records of an event
/// that already happened, and they never advance or surface. Called when a
/// fact transitions to `Superseded`; idempotent (no-op for facts without an
/// overlay or pending shape), so it is safe on every supersession path.
pub(crate) async fn retire_overlay_for_fact_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fact_id: i32,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    sqlx::query(
        "UPDATE events SET status_id = ?, addressed_at = COALESCE(?, addressed_at) \
         WHERE fact_id = ? AND status_id NOT IN (?, ?)",
    )
    .bind(EventStatus::Dismissed as i16)
    .bind(now)
    .bind(fact_id)
    .bind(EventStatus::Completed as i16)
    .bind(EventStatus::Dismissed as i16)
    .execute(&mut **tx)
    .await?;

    sqlx::query("DELETE FROM pending_event_meta WHERE fact_id = ?")
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}
