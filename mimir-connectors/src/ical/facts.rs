//! VEVENT → `NormalizedFact` cluster extraction (shared by Calendar and
//! Email connectors).

use chrono::{DateTime, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::NormalizedFact;
use tracing::debug;

use crate::ical::RawVEvent;

pub fn vevent_to_facts(
    user_identity: Option<&str>,
    event: &RawVEvent,
    raw_reference: &str,
) -> Vec<NormalizedFact> {
    let Some(start) = event.starts_at else {
        debug!(raw_reference, "skipping event with no parseable DTSTART");
        return Vec::new();
    };
    // The event entity is named by its SUMMARY (falling back to the UID) so
    // the primary, location, and attendee facts all resolve to the same
    // `Event` entity.
    let event_name = match non_empty(event.summary.as_deref()) {
        Some(name) => name.to_string(),
        None => match non_empty(event.uid.as_deref()) {
            Some(uid) => uid.to_string(),
            None => {
                debug!(raw_reference, "skipping event with no summary or uid");
                return Vec::new();
            }
        },
    };
    let recurrence = rrule_to_recurrence(event.recurrence_rule.as_deref());
    // A recurring event keeps surfacing on every occurrence, so its fact must
    // not expire after the first instance's `DTEND`. Leaving `valid_until`
    // unset keeps the fact live for current-facts reads and supersession; a
    // one-time event still carries its `DTEND` bound.
    let valid_until = (recurrence == RecurrenceType::None)
        .then_some(event.ends_at)
        .flatten();

    let mut facts = Vec::new();

    // 1. Primary appointment fact (user-scoped when an identity is set).
    if let Some(user) = user_identity {
        facts.push(vevent_fact(
            user.to_string(),
            EntityType::Person,
            "has_event",
            event_name.clone(),
            Some(EntityType::Event),
            Some(start),
            valid_until,
            recurrence,
            Some(EventType::Appointment),
            raw_reference,
        ));
    }

    // 2. Location → Place entity (resolved via F5). Carries no temporal
    //    bounds: a venue is a property of the event, not a trigger, so it must
    //    not spawn its own events-subsystem overlay.
    if let Some(loc) = non_empty(event.location.as_deref()) {
        facts.push(vevent_fact(
            event_name.clone(),
            EntityType::Event,
            "located_in",
            loc.to_string(),
            Some(EntityType::Place),
            None,
            None,
            RecurrenceType::None,
            None,
            raw_reference,
        ));
    }

    // 3. Attendees → Person entities (resolved via F5). Like the location fact,
    //    attendance is a relationship, not a trigger, so it carries no temporal
    //    bounds and spawns no overlay.
    for attendee in &event.attendees {
        facts.push(vevent_fact(
            attendee.clone(),
            EntityType::Person,
            "attending",
            event_name.clone(),
            Some(EntityType::Event),
            None,
            None,
            RecurrenceType::None,
            None,
            raw_reference,
        ));
    }

    facts
}

// ---------------------------------------------------------------------------
// iCalendar value helpers
// ---------------------------------------------------------------------------

/// Parse an iCalendar `DATE-TIME` / `DATE` value into UTC.
///
/// Forms handled (RFC 5545 §3.3.5): UTC (`20250503T090000Z`), floating local
/// time (treated as UTC — the "FORM #1" red-flag case, rare in real exports),
/// date-only (`20250503` → midnight UTC), and `TZID`-qualified local
/// (`DTSTART;TZID=Europe/London:20250503T090000`). The latter is resolved via
/// `chrono-tz`; an *unknown* zone or an ambiguous local time (rare DST fold)
/// falls back to the naive value read as UTC so a bad `TZID` never silently
/// drops the event. Returns `None` only when the value cannot be parsed at all.
fn rrule_to_recurrence(rrule: Option<&str>) -> RecurrenceType {
    let Some(rule) = rrule else {
        return RecurrenceType::None;
    };
    for part in rule.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("FREQ") {
            return match value.trim().to_ascii_uppercase().as_str() {
                "DAILY" => RecurrenceType::Daily,
                "WEEKLY" => RecurrenceType::Weekly,
                "MONTHLY" => RecurrenceType::Monthly,
                "YEARLY" => RecurrenceType::Yearly,
                _ => RecurrenceType::None,
            };
        }
    }
    RecurrenceType::None
}

/// Trim a string and return it only when non-empty (after trimming).
fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|t| !t.is_empty())
}

/// Build a VEVENT [`NormalizedFact`] with the shared connector defaults filled
/// in: connector source type, non-sensitive, non-correction, no category ids,
/// no location overlay.
///
/// All three VEVENT fact shapes (`has_event` / `located_in` / `attending`)
/// share these defaults; the per-shape fields (subject, relationship, object,
/// recurrence, event-type hint) are the arguments. Collapsing the struct
/// literals here keeps the extractor readable and ensures the connector-level
/// invariants (source type, sensitivity, raw reference) stay in one place.
#[allow(clippy::too_many_arguments)] // constructor helper: every arg maps to a `NormalizedFact` field
pub(crate) fn vevent_fact(
    subject: String,
    subject_type: EntityType,
    relationship_type: &str,
    object: String,
    object_type: Option<EntityType>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    recurrence: RecurrenceType,
    event_type: Option<EventType>,
    raw_ref: &str,
) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::Connector,
        subject,
        subject_type,
        relationship_type: relationship_type.to_string(),
        object,
        object_is_entity: true,
        object_type,
        valid_from,
        valid_until,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence,
        requires_user_action: false,
        raw_reference: Some(raw_ref.to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        event_type,
        location: None,
    }
}
