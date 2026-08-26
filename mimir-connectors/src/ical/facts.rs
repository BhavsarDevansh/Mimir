//! VEVENT → `NormalizedFact` cluster extraction (shared by Calendar and
//! Email connectors).

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::NormalizedFact;
use tracing::debug;

use crate::fact::connector_fact;
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
    let cadence = parse_rrule(event.recurrence_rule.as_deref(), start);
    let recurrence = cadence.kind;
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
        facts.push(connector_fact(
            user.to_string(),
            EntityType::Person,
            "has_event",
            event_name.clone(),
            true,
            Some(EntityType::Event),
            Some(start),
            valid_until,
            recurrence,
            event.recurrence_rule.clone(),
            cadence.interval,
            cadence.until,
            raw_reference,
            Some(ExtractionMethod::StructuredParse),
            Some(EventType::Appointment),
            None,
        ));
    }

    // 2. Location → Place entity (resolved via F5). Carries no temporal
    //    bounds: a venue is a property of the event, not a trigger, so it must
    //    not spawn its own events-subsystem overlay.
    if let Some(loc) = non_empty(event.location.as_deref()) {
        facts.push(connector_fact(
            event_name.clone(),
            EntityType::Event,
            "located_in",
            loc.to_string(),
            true,
            Some(EntityType::Place),
            None,
            None,
            RecurrenceType::None,
            None,
            1,
            None,
            raw_reference,
            Some(ExtractionMethod::StructuredParse),
            None,
            None,
        ));
    }

    // 3. Attendees → Person entities (resolved via F5). Like the location fact,
    //    attendance is a relationship, not a trigger, so it carries no temporal
    //    bounds and spawns no overlay.
    for attendee in &event.attendees {
        facts.push(connector_fact(
            attendee.clone(),
            EntityType::Person,
            "attending",
            event_name.clone(),
            true,
            Some(EntityType::Event),
            None,
            None,
            RecurrenceType::None,
            None,
            1,
            None,
            raw_reference,
            Some(ExtractionMethod::StructuredParse),
            None,
            None,
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
/// The recurrence cadence parsed from an `RRULE`: the kind, the interval
/// (every N periods), and the effective series end.
struct RruleCadence {
    kind: RecurrenceType,
    interval: i32,
    until: Option<DateTime<Utc>>,
}

/// Parse an `RRULE` into its cadence. `FREQ` drives the [`RecurrenceType`];
/// `INTERVAL` repeats the period every N steps (2 = fortnightly for
/// `WEEKLY`); `UNTIL` bounds the series directly and `COUNT` bounds it via
/// the series start (the COUNT-th occurrence is `(COUNT-1) * INTERVAL`
/// periods after it), so a bounded series no longer stays active
/// indefinitely. Unknown or missing `FREQ` maps to [`RecurrenceType::None`]
/// (no recurrence).
fn parse_rrule(rrule: Option<&str>, series_start: DateTime<Utc>) -> RruleCadence {
    let Some(rule) = rrule else {
        return RruleCadence {
            kind: RecurrenceType::None,
            interval: 1,
            until: None,
        };
    };
    let mut kind = RecurrenceType::None;
    let mut interval = 1i32;
    let mut count: Option<u32> = None;
    let mut until: Option<DateTime<Utc>> = None;
    for part in rule.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                kind = match value.trim().to_ascii_uppercase().as_str() {
                    "DAILY" => RecurrenceType::Daily,
                    "WEEKLY" => RecurrenceType::Weekly,
                    "MONTHLY" => RecurrenceType::Monthly,
                    "YEARLY" => RecurrenceType::Yearly,
                    _ => RecurrenceType::None,
                };
            }
            "INTERVAL" => {
                if let Ok(n) = value.trim().parse::<i32>() {
                    interval = n.max(1);
                }
            }
            "COUNT" => {
                if let Ok(n) = value.trim().parse::<u32>() {
                    count = Some(n);
                }
            }
            "UNTIL" => until = parse_rrule_until(value.trim()),
            _ => {}
        }
    }
    let until = until.or_else(|| {
        let count = count?;
        series_end_from_count(series_start, kind, interval, count)
    });
    RruleCadence {
        kind,
        interval,
        until,
    }
}

/// Parse an `RRULE` `UNTIL` value (`YYYYMMDDTHHMMSSZ` or date-only
/// `YYYYMMDD` → midnight UTC).
fn parse_rrule_until(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
    Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?))
}

/// The datetime of the COUNT-th occurrence: `(COUNT-1) * INTERVAL` periods
/// after the series start (the first occurrence is the start itself).
fn series_end_from_count(
    series_start: DateTime<Utc>,
    kind: RecurrenceType,
    interval: i32,
    count: u32,
) -> Option<DateTime<Utc>> {
    let steps = (count.saturating_sub(1)) as i64 * interval.max(1) as i64;
    let date = match kind {
        RecurrenceType::Daily => series_start
            .date_naive()
            .checked_add_days(chrono::Days::new(steps as u64))?,
        RecurrenceType::Weekly => series_start
            .date_naive()
            .checked_add_days(chrono::Days::new(steps.saturating_mul(7) as u64))?,
        RecurrenceType::Monthly => series_start
            .date_naive()
            .checked_add_months(chrono::Months::new(steps as u32))?,
        RecurrenceType::Yearly => series_start
            .date_naive()
            .checked_add_months(chrono::Months::new(steps.saturating_mul(12) as u32))?,
        RecurrenceType::None => return None,
    };
    Utc.from_local_datetime(&date.and_time(series_start.time()))
        .single()
}

/// Trim a string and return it only when non-empty (after trimming).
fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|t| !t.is_empty())
}
