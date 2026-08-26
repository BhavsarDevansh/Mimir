//! `events.upcoming_scan` job — derives event overlays and applies deterministic
//! auto-complete policies (issue #74).
//!
//! All logic is deterministic Rust; no LLM is involved. The scan runs in three
//! passes:
//!
//! 1. **Derive** — facts with a future `valid_from` and no overlay get a
//!    one-time `AutoCompleteOnDate` overlay.
//! 2. **Auto-complete** — one-time `AutoCompleteOnDate` events whose
//!    `trigger_date` has passed transition to `Completed`.
//! 3. **Advance** — recurring events whose `trigger_date` has passed advance
//!    to their next occurrence via [`next_occurrence`].
//!
//! `RequiresUserAction` events are intentionally left untouched; once past
//! their `trigger_date` they surface as overdue via
//! [`crate::queries::event::get_overdue_events`].

use crate::KnowledgeError;
use crate::KnowledgeGraph;
use crate::models::enums::{AutoCompletePolicy, EventStatus, EventType, RecurrenceType};
use crate::models::event::NewEvent;
use crate::models::recurrence::next_occurrence;
use crate::queries::event;

/// Summary of a single scan run, for logging and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanSummary {
    /// One-time overlays created for newly-discovered future facts.
    pub derived: usize,
    /// One-time events transitioned to `Completed`.
    pub completed: usize,
    /// Recurring events advanced to their next occurrence.
    pub advanced: usize,
}

/// Run the upcoming-scan over the knowledge graph.
///
/// `horizon_days` bounds how far into the future the derive pass looks for
/// future-dated facts without an overlay.
pub async fn run_upcoming_scan(
    kg: &KnowledgeGraph,
    horizon_days: i64,
) -> Result<ScanSummary, KnowledgeError> {
    let pool = kg.pool();
    let now = kg.now();
    let horizon = now + chrono::Duration::days(horizon_days);

    // 1. Derive overlays for future-dated facts that lack one.
    let future = event::get_future_facts_without_overlay(pool, now, horizon).await?;
    let mut derived = 0usize;
    for ff in future {
        let new = NewEvent {
            fact_id: ff.fact_id,
            entity_id: ff.entity_id,
            trigger_date: ff.valid_from,
            recurrence: RecurrenceType::None,
            recurrence_rule: None,
            recurrence_interval: 1,
            recurrence_until: None,
            event_type: EventType::Reminder,
            auto_complete_policy: AutoCompletePolicy::AutoCompleteOnDate,
            requires_user_action: false,
        };
        // Idempotent: a concurrent writer (e.g. extraction) may create the
        // same overlay between the select and insert. Only count actual inserts.
        if event::insert_event_if_absent(pool, &new).await?.is_some() {
            derived += 1;
        }
    }

    // 2. Auto-complete one-time events past their trigger date.
    let past_due = event::get_past_due_auto_complete(pool, now).await?;
    let mut completed = 0usize;
    for ev in past_due {
        event::update_status(pool, ev.id, EventStatus::Completed, now).await?;
        completed += 1;
    }

    // 3. Advance recurring events whose trigger date has passed. The SQL filter
    // (`trigger_date < now`) is pushed into `get_active_recurring` so only rows
    // that can actually advance are loaded and sorted.
    let recurring = event::get_active_recurring(pool, now).await?;
    let mut advanced = 0usize;
    for ev in recurring {
        let recurrence = ev.recurrence().unwrap_or(RecurrenceType::None);
        if let Some(next) = next_occurrence(
            &ev.trigger_date.to_rfc3339(),
            recurrence,
            ev.recurrence_interval,
            ev.recurrence_until,
            now,
        ) {
            if next != ev.trigger_date {
                event::advance_recurring_trigger(pool, ev.id, next).await?;
                advanced += 1;
            }
        }
    }

    Ok(ScanSummary {
        derived,
        completed,
        advanced,
    })
}
