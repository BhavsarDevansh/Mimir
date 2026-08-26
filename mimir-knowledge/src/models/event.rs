//! Event overlay model for the events & reminders subsystem (issue #74).
//!
//! An [`Event`] is a lifecycle + recurrence overlay attached to a fact. A fact
//! whose `valid_from` lies in the future is a one-time event; a fact tagged with
//! recurrence (e.g. a birthday) is a recurring event. The source fact surfaces
//! in the "Upcoming" memory section; the overlay only manages lifecycle status
//! and recurrence advancement.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::enums::{AutoCompletePolicy, EventStatus, EventType, RecurrenceType};

/// A row in the `events` table — the lifecycle overlay on a fact.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Event {
    pub id: i32,
    pub fact_id: i32,
    pub entity_id: i32,
    pub trigger_date: DateTime<Utc>,
    pub recurrence_type_id: i16,
    /// Raw `RRULE` string (interval, day/month constraints, `COUNT`/`UNTIL`)
    /// when the producer supplied one; `None` for kind-only producers.
    pub recurrence_rule: Option<String>,
    /// How often the series repeats (every N periods; 1 = every period).
    pub recurrence_interval: i32,
    /// Effective series end (from `UNTIL`, or computed from `COUNT` at
    /// extraction); `None` = unbounded.
    pub recurrence_until: Option<DateTime<Utc>>,
    pub event_type_id: i16,
    pub status_id: i16,
    pub auto_complete_policy_id: i16,
    pub requires_user_action: bool,
    pub addressed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Event {
    /// Typed recurrence kind, or `None` if the stored id is unknown.
    pub fn recurrence(&self) -> Option<RecurrenceType> {
        RecurrenceType::try_from(self.recurrence_type_id).ok()
    }

    /// Typed event kind, or `None` if the stored id is unknown.
    pub fn event_type(&self) -> Option<EventType> {
        EventType::try_from(self.event_type_id).ok()
    }

    /// Typed lifecycle status, or `None` if the stored id is unknown.
    pub fn status(&self) -> Option<EventStatus> {
        EventStatus::try_from(self.status_id).ok()
    }

    /// Typed auto-complete policy, or `None` if the stored id is unknown.
    pub fn policy(&self) -> Option<AutoCompletePolicy> {
        AutoCompletePolicy::try_from(self.auto_complete_policy_id).ok()
    }

    /// Whether this overlay represents a recurring event.
    pub fn is_recurring(&self) -> bool {
        self.recurrence_type_id != RecurrenceType::None as i16
    }
}

/// Data needed to create a new event overlay.
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub fact_id: i32,
    pub entity_id: i32,
    pub trigger_date: DateTime<Utc>,
    pub recurrence: RecurrenceType,
    /// Raw `RRULE` string when the producer supplied one.
    pub recurrence_rule: Option<String>,
    /// How often the series repeats (every N periods; 1 = every period).
    pub recurrence_interval: i32,
    /// Effective series end; `None` = unbounded.
    pub recurrence_until: Option<DateTime<Utc>>,
    pub event_type: EventType,
    pub auto_complete_policy: AutoCompletePolicy,
    pub requires_user_action: bool,
}
