//! Event-subsystem overlay derivation from normalized facts (issue #74).

use chrono::{DateTime, Utc};

use crate::models::enums::{AutoCompletePolicy, EventType, RecurrenceType};
use crate::models::event::NewEvent;
// ---------------------------------------------------------------------------

/// Build an event overlay from a normalized fact, if it qualifies.
///
/// Qualification (deterministic, issue #74): the fact has a `valid_from`
/// (trigger date) AND at least one of: `valid_from` is in the future, the
/// recurrence is non-`None`, or `requires_user_action` is set.
pub(super) fn event_from_extraction(
    recurrence: RecurrenceType,
    requires_user_action: bool,
    entity_id: i32,
    fact_id: i32,
    valid_from: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    event_type_hint: Option<EventType>,
) -> Option<NewEvent> {
    let trigger_date = valid_from?;

    let is_future = trigger_date > now;
    if !is_future && recurrence == RecurrenceType::None && !requires_user_action {
        return None;
    }

    let auto_complete_policy = if recurrence != RecurrenceType::None {
        AutoCompletePolicy::Recurring
    } else if requires_user_action {
        AutoCompletePolicy::RequiresUserAction
    } else {
        AutoCompletePolicy::AutoCompleteOnDate
    };
    // The producer may hint the event kind (e.g. a Calendar connector sets
    // `Appointment`). When present the hint wins; without one the original
    // conversational derivation applies (`Task` for an action item,
    // `Reminder` otherwise), so chat behaviour is unchanged.
    let event_type = event_type_hint.unwrap_or(if requires_user_action {
        EventType::Task
    } else {
        EventType::Reminder
    });

    Some(NewEvent {
        fact_id,
        entity_id,
        trigger_date,
        recurrence,
        event_type,
        auto_complete_policy,
        requires_user_action,
    })
}

// ---------------------------------------------------------------------------
