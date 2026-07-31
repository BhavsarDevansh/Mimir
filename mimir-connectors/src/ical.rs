//! Shared iCalendar VEVENT parsing + fact extraction (Phase 3 C4 / #198 and
//! C6 / #200).
//!
//! Both connector backends that consume iCalendar data — the CalDAV Calendar
//! connector (`#197`/`#198`) and the IMAP Email connector's iMIP-invite
//! extraction (`#200`) — parse the *same* RFC 5545 VEVENT payload into the
//! *same* intrinsic fields and turn it into the *same* cluster of
//! [`NormalizedFact`]s. This module is the single source of truth for both:
//!
//! - [`parse_ical_to_vevents`] parses an iCalendar text payload into one
//!   [`RawVEvent`] per `VEVENT` (UTC-resolved dates, resolved attendee display
//!   names, raw `RRULE`). It does not know where the payload came from
//!   (a CalDAV resource or a `text/calendar` MIME part) — that provenance is
//!   supplied by the caller as the `raw_reference` on each fact.
//! - [`vevent_to_facts`] turns one [`RawVEvent`] into the appointment cluster:
//!   `user has_event <event>` (typed [`EventType::Appointment`], recurrence
//!   from `RRULE` `FREQ`), `<event> located_in <place>`, and
//!   `<attendee> attending <event>`. Entity resolution, confidence, and the
//!   events-subsystem overlay are left to the shared `normalize_and_insert`
//!   pipeline; this function only builds the typed facts.
//!
//! Gated by `any(feature = "calendar", feature = "gmail")`: it is needed only
//! by the two connector backends that consume iCalendar. The parsing helpers
//! depend on `icalendar` (the `parser` submodule) and `chrono-tz`, both made
//! available to either backend by their feature flags.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::NormalizedFact;
use tracing::debug;

// ---------------------------------------------------------------------------
// Intrinsic parsed VEVENT
// ---------------------------------------------------------------------------

/// A parsed iCalendar `VEVENT`'s intrinsic fields — the data both the CalDAV
/// Calendar connector and the IMAP Email iMIP extraction need.
///
/// Dates are parsed to UTC at staging time: `DTSTART`/`DTEND` may be UTC,
/// floating local, date-only, or `TZID`-qualified; the latter is resolved via
/// `chrono-tz`. `attendees` carry resolved display names (the `CN` parameter,
/// else the `mailto:` value) so the fact extractor can resolve them to `Person`
/// entities via the full F5 chain. `recurrence_rule` stays a raw `RRULE`
/// string; [`vevent_to_facts`] maps `FREQ` to [`RecurrenceType`] so the
/// existing events-subsystem recurrence logic advances recurring events.
///
/// CalDAV-specific provenance (the resource `href` and `etag`) is *not*
/// intrinsic — the Calendar connector wraps this in a `RawCalDavEvent`
/// alongside those fields, while the Email connector uses this type directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVEvent {
    /// `UID` property.
    pub uid: Option<String>,
    /// `SUMMARY` property.
    pub summary: Option<String>,
    /// Parsed `DTSTART` resolved to UTC. `None` when the value is absent or
    /// unparseable ([`vevent_to_facts`] skips events without a start).
    pub starts_at: Option<DateTime<Utc>>,
    /// Parsed `DTEND` resolved to UTC. `None` when absent (an all-day or
    /// unbounded event); the extractor treats the event as point-in-time.
    pub ends_at: Option<DateTime<Utc>>,
    /// `LOCATION` property.
    pub location: Option<String>,
    /// `DESCRIPTION` property.
    pub description: Option<String>,
    /// `STATUS` property (e.g. `CONFIRMED` / `CANCELLED`). Not used by fact
    /// extraction today; retained for a future CANCEL lifecycle / status pass.
    pub status: Option<String>,
    /// `RRULE` property (recurrence; the events-subsystem #74 owns this).
    pub recurrence_rule: Option<String>,
    /// Resolved display names of every `ATTENDEE` (the `CN` parameter, else
    /// the `mailto:` value), in document order.
    pub attendees: Vec<String>,
    /// Resolved display name of the `ORGANIZER`, if present.
    pub organizer: Option<String>,
}

/// Parse an iCalendar payload into the `VEVENT`s it contains.
///
/// Returns one [`RawVEvent`] per `VEVENT`. An empty/invalid payload yields an
/// empty vec (the connector logs and skips rather than failing the sync), so
/// one malformed event never aborts a whole sync.
pub fn parse_ical_to_vevents(ical: &str) -> Vec<RawVEvent> {
    // The low-level parser (`icalendar::parser`) yields a zero-copy
    // `Calendar` whose top-level `components` are the VEVENT/VTODO/VTIMEZONE
    // entries. We walk the low-level representation directly (the high-level
    // `icalendar::Calendar` is builder-oriented and has no parse-from-str
    // path in 0.17.x). `find_prop` + `ParseString::as_str` give owned copies
    // so the staged events outlive the borrowed input; `params` expose the
    // `TZID`/`CN` parameters the extractors need for UTC resolution and
    // attendee names.
    use icalendar::parser::read_calendar;
    let Ok(calendar) = read_calendar(ical) else {
        return Vec::new();
    };
    calendar
        .components
        .iter()
        .filter(|c| c.name.as_str() == "VEVENT")
        .map(|event| {
            let prop_str = |key: &str| event.find_prop(key).map(|p| p.val.as_str().to_string());
            // Read a property's `TZID` parameter (case-insensitive key).
            let tzid_of = |key: &str| {
                event.find_prop(key).and_then(|p| {
                    p.params
                        .iter()
                        .find(|q| q.key.as_ref().eq_ignore_ascii_case("TZID"))
                        .and_then(|q| q.val.as_ref().map(|v| v.as_str().to_string()))
                })
            };
            let starts_at = event
                .find_prop("DTSTART")
                .and_then(|p| parse_ical_datetime(p.val.as_str(), tzid_of("DTSTART").as_deref()));
            let ends_at = event
                .find_prop("DTEND")
                .and_then(|p| parse_ical_datetime(p.val.as_str(), tzid_of("DTEND").as_deref()));
            let attendees = event
                .properties
                .iter()
                .filter(|p| p.name.as_ref().eq_ignore_ascii_case("ATTENDEE"))
                .filter_map(participant_display)
                .collect();
            let organizer = event.find_prop("ORGANIZER").and_then(participant_display);
            RawVEvent {
                uid: prop_str("UID"),
                summary: prop_str("SUMMARY"),
                starts_at,
                ends_at,
                location: prop_str("LOCATION"),
                description: prop_str("DESCRIPTION"),
                status: prop_str("STATUS"),
                recurrence_rule: prop_str("RRULE"),
                attendees,
                organizer,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VEVENT → NormalizedFact cluster
// ---------------------------------------------------------------------------

/// Turn one parsed [`RawVEvent`] into its cluster of [`NormalizedFact`]s.
///
/// Emits up to three fact shapes, all resolved by the shared
/// `normalize_and_insert` pipeline:
/// 1. `user has_event <event>` — the primary appointment, carrying the
///    temporal bounds, the recurrence (`RRULE` `FREQ`), and an
///    [`EventType::Appointment`] hint so the events-subsystem overlay is
///    typed correctly. Authored only when a user identity is supplied (so the
///    event surfaces in the user's "Upcoming" section); without one the
///    primary fact is skipped and the event is captured via its
///    location/attendee facts instead.
/// 2. `<event> located_in <place>` — the `LOCATION` resolves to a `Place`
///    entity via the full F5 chain (no `entity_locations` overlay; a venue is
///    a property of the event, not the user's location history). Carries no
///    temporal bounds, so it spawns no events-subsystem overlay.
/// 3. `<attendee> attending <event>` — each `ATTENDEE` resolves to a `Person`
///    entity via F5. Like the location fact it carries no temporal bounds and
///    spawns no overlay.
///
/// `raw_reference` is the native id of the source item (a CalDAV VEVENT `UID`,
/// or an iMIP invite's email UID). It rides on every fact as the provenance
/// `raw_reference` so the KB can trace each fact back to its source.
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
fn parse_ical_datetime(value: &str, tzid: Option<&str>) -> Option<DateTime<Utc>> {
    const NAIVE_FMT: &str = "%Y%m%dT%H%M%S";
    const DATE_FMT: &str = "%Y%m%d";

    // UTC form (`20250503T090000Z`): strip the trailing `Z`, parse the naive
    // datetime, and label it UTC. Avoids the deprecated
    // `TimeZone::datetime_from_str`.
    if let Some(rest) = value.strip_suffix('Z') {
        if let Ok(naive) = NaiveDateTime::parse_from_str(rest, NAIVE_FMT) {
            return Some(naive.and_utc());
        }
    }
    if let Some(tz) = tzid {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, NAIVE_FMT) {
            if let Ok(zone) = tz.parse::<chrono_tz::Tz>() {
                // An ambiguous autumn-fold local time resolves to a single
                // instant when unambiguous; otherwise prefer the earliest
                // offset (keeping the value within an hour of the wall clock)
                // before falling back to naive-as-UTC. A spring-forward gap
                // (`None` from both) still hits the fallback below.
                let local = zone
                    .from_local_datetime(&naive)
                    .single()
                    .or_else(|| zone.from_local_datetime(&naive).earliest());
                if let Some(local) = local {
                    return Some(local.with_timezone(&Utc));
                }
            }
            // Unknown zone or genuinely ambiguous/unrepresentable local time:
            // read the naive value as UTC so a bad `TZID` never drops the event.
            return Some(naive.and_utc());
        }
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, NAIVE_FMT) {
        return Some(naive.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, DATE_FMT) {
        let naive = date.and_hms_opt(0, 0, 0)?;
        return Utc.from_local_datetime(&naive).single();
    }
    None
}

/// Extract a human display name from an `ATTENDEE`/`ORGANIZER` property.
///
/// Prefers the `CN` ("common name") parameter (surrounding quotes stripped);
/// otherwise strips a `mailto:` scheme from the value. Returns `None` for an
/// empty result so it is naturally filtered out of the attendees list.
fn participant_display(prop: &icalendar::parser::Property<'_>) -> Option<String> {
    let cn = prop
        .params
        .iter()
        .find(|p| p.key.as_ref().eq_ignore_ascii_case("CN"))
        .and_then(|p| p.val.as_ref().map(|v| v.as_str().to_string()))
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());
    if let Some(name) = cn {
        return Some(name);
    }
    let val = prop.val.as_str();
    let name = val.strip_prefix("mailto:").unwrap_or(val).trim();
    let name = name.trim_matches('"');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Map an iCalendar `RRULE` to the coarse [`RecurrenceType`] the
/// events-subsystem (#74) advances.
///
/// Only the `FREQ` part maps: the existing recurrence engine is a per-`FREQ`
/// next-occurrence model, so `COUNT`, `UNTIL`, `INTERVAL`, and `BYxxx` parts
/// are out of scope (a calendar `RRULE` is far richer than the KB's recurrence
/// axis). An absent or unparseable `RRULE` (and an unknown `FREQ`) yield
/// [`RecurrenceType::None`] — the event is treated as one-time, which the
/// events-subsystem auto-completes once its date passes.
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
fn vevent_fact(
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
        event_type,
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn parse_ical_datetime_utc_date_only_and_floating() {
        assert_eq!(
            parse_ical_datetime("20250503T090000Z", None),
            Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap())
        );
        // Date-only → midnight UTC.
        assert_eq!(
            parse_ical_datetime("20250503", None),
            Some(Utc.with_ymd_and_hms(2025, 5, 3, 0, 0, 0).unwrap())
        );
        // Floating local (no Z, no TZID) is read as UTC.
        assert_eq!(
            parse_ical_datetime("20250503T090000", None),
            Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap())
        );
        assert!(parse_ical_datetime("not-a-date", None).is_none());
    }

    #[test]
    fn parse_ical_datetime_tzid_resolves_with_dst() {
        // 09:00 Europe/London on 2025-07-03 is BST (+01:00) → 08:00 UTC.
        assert_eq!(
            parse_ical_datetime("20250703T090000", Some("Europe/London")),
            Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 0, 0).unwrap())
        );
        // Winter: 09:00 GMT → 09:00 UTC.
        assert_eq!(
            parse_ical_datetime("20250103T090000", Some("Europe/London")),
            Some(Utc.with_ymd_and_hms(2025, 1, 3, 9, 0, 0).unwrap())
        );
        // An unknown TZID falls back to the naive value read as UTC (event
        // is not silently dropped).
        assert_eq!(
            parse_ical_datetime("20250103T090000", Some("Mars/Olympus")),
            Some(Utc.with_ymd_and_hms(2025, 1, 3, 9, 0, 0).unwrap())
        );
    }

    #[test]
    fn parse_ical_datetime_tzid_autumn_fold_prefers_earliest_offset() {
        // The Europe/London clocks-back fold on 2025-10-26 makes 01:30 local
        // ambiguous: it occurs once under BST (+01:00 → 00:30 UTC) and again
        // under GMT (+00:00 → 01:30 UTC). The earliest offset is preferred so
        // the event stays within an hour of the wall clock rather than
        // shifting by the full zone offset via the naive-as-UTC fallback.
        assert_eq!(
            parse_ical_datetime("20251026T013000", Some("Europe/London")),
            Some(Utc.with_ymd_and_hms(2025, 10, 26, 0, 30, 0).unwrap())
        );
    }

    #[test]
    fn parse_ical_to_vevents_extracts_fields_and_recur() {
        const ICAL_EVENT: &str = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
PRODID:-//Mimir//Test//EN\n\
BEGIN:VEVENT\n\
UID:uid-1@test\n\
SUMMARY:Trip to Rome\n\
DTSTART:20250503T090000Z\n\
DTEND:20250507T180000Z\n\
LOCATION:Rome\n\
STATUS:CONFIRMED\n\
END:VEVENT\n\
END:VCALENDAR";
        let events = parse_ical_to_vevents(ICAL_EVENT);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.uid.as_deref(), Some("uid-1@test"));
        assert_eq!(e.summary.as_deref(), Some("Trip to Rome"));
        assert_eq!(
            e.starts_at,
            Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap())
        );
        assert_eq!(
            e.ends_at,
            Some(Utc.with_ymd_and_hms(2025, 5, 7, 18, 0, 0).unwrap())
        );
        assert!(e.attendees.is_empty());
        assert!(e.organizer.is_none());
        assert_eq!(e.location.as_deref(), Some("Rome"));
        assert_eq!(e.status.as_deref(), Some("CONFIRMED"));
        assert!(e.recurrence_rule.is_none());

        const ICAL_RECURRING: &str = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
BEGIN:VEVENT\n\
UID:bday@test\n\
SUMMARY:Mom's birthday\n\
DTSTART:20250101T090000Z\n\
RRULE:FREQ=YEARLY\n\
END:VEVENT\n\
END:VCALENDAR";
        let rec = parse_ical_to_vevents(ICAL_RECURRING);
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].recurrence_rule.as_deref(), Some("FREQ=YEARLY"));
    }

    #[test]
    fn parse_ical_to_vevents_invalid_payload_returns_empty() {
        assert!(parse_ical_to_vevents("not ical at all").is_empty());
        assert!(parse_ical_to_vevents("").is_empty());
    }

    #[test]
    fn parse_ical_to_vevents_extracts_attendees_organizer_and_tzid() {
        const ICAL: &str = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
BEGIN:VEVENT\n\
UID:meet-1@test\n\
SUMMARY:Standup\n\
DTSTART;TZID=Europe/London:20250703T090000\n\
DTEND;TZID=Europe/London:20250703T093000\n\
ORGANIZER;CN=Devansh Bhavsar:mailto:devansh@example.com\n\
ATTENDEE;CN=Alice;ROLE=REQ-PARTICIPANT:mailto:alice@example.com\n\
ATTENDEE:mailto:bob@example.com\n\
ATTENDEE;CN=:mailto:empty@example.com\n\
END:VEVENT\n\
END:VCALENDAR";
        let events = parse_ical_to_vevents(ICAL);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(
            e.starts_at,
            Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 0, 0).unwrap())
        );
        assert_eq!(
            e.ends_at,
            Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 30, 0).unwrap())
        );
        assert_eq!(e.organizer.as_deref(), Some("Devansh Bhavsar"));
        // CN present → name; no CN → mailto value; empty CN → mailto value.
        assert_eq!(
            e.attendees,
            vec!["Alice", "bob@example.com", "empty@example.com"]
        );
    }

    #[test]
    fn vevent_to_facts_emits_primary_location_and_attendee_facts() {
        let event = RawVEvent {
            uid: Some("uid-1@test".into()),
            summary: Some("Trip to Rome".into()),
            starts_at: Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap()),
            ends_at: Some(Utc.with_ymd_and_hms(2025, 5, 7, 18, 0, 0).unwrap()),
            location: Some("Rome".into()),
            description: None,
            status: Some("CONFIRMED".into()),
            recurrence_rule: None,
            attendees: vec!["Alice".into(), "bob@example.com".into()],
            organizer: Some("Devansh Bhavsar".into()),
        };
        let facts = vevent_to_facts(Some("Devansh"), &event, "uid-1@test");
        // 1 primary (has_event) + 1 location + 2 attendees = 4.
        assert_eq!(facts.len(), 4);
        let primary = &facts[0];
        assert_eq!(primary.subject, "Devansh");
        assert_eq!(primary.subject_type, EntityType::Person);
        assert_eq!(primary.relationship_type, "has_event");
        assert_eq!(primary.object, "Trip to Rome");
        assert_eq!(primary.object_type, Some(EntityType::Event));
        assert!(primary.valid_from.is_some());
        assert!(primary.valid_until.is_some());
        assert_eq!(primary.event_type, Some(EventType::Appointment));
        assert_eq!(primary.raw_reference.as_deref(), Some("uid-1@test"));
        // Location fact carries no temporal bounds (no overlay).
        let loc = &facts[1];
        assert_eq!(loc.relationship_type, "located_in");
        assert_eq!(loc.object, "Rome");
        assert_eq!(loc.object_type, Some(EntityType::Place));
        assert!(loc.valid_from.is_none());
        assert!(loc.valid_until.is_none());
        assert!(loc.event_type.is_none());
        // Attendee facts carry no temporal bounds (no overlay).
        assert_eq!(facts[2].relationship_type, "attending");
        assert_eq!(facts[2].subject, "Alice");
        assert_eq!(facts[2].object_type, Some(EntityType::Event));
        assert!(facts[2].valid_from.is_none());
        assert_eq!(facts[3].subject, "bob@example.com");
    }

    #[test]
    fn vevent_to_facts_skips_primary_when_no_user_identity() {
        let event = RawVEvent {
            uid: Some("uid-1@test".into()),
            summary: Some("Trip to Rome".into()),
            starts_at: Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap()),
            ends_at: None,
            location: None,
            description: None,
            status: None,
            recurrence_rule: None,
            attendees: vec![],
            organizer: None,
        };
        // No user identity → no primary has_event fact; event is still captured
        // via location/attendee facts (none here), so the cluster is empty.
        let facts = vevent_to_facts(None, &event, "uid-1@test");
        assert!(facts.is_empty());
    }

    #[test]
    fn vevent_to_facts_skips_event_with_no_dtstart() {
        let event = RawVEvent {
            uid: Some("uid-1@test".into()),
            summary: Some("Trip to Rome".into()),
            starts_at: None,
            ends_at: None,
            location: Some("Rome".into()),
            description: None,
            status: None,
            recurrence_rule: None,
            attendees: vec!["Alice".into()],
            organizer: None,
        };
        let facts = vevent_to_facts(Some("Devansh"), &event, "uid-1@test");
        assert!(facts.is_empty(), "no DTSTART → no facts (event skipped)");
    }

    #[test]
    fn vevent_to_facts_recurring_event_has_no_valid_until() {
        let event = RawVEvent {
            uid: Some("standup@test".into()),
            summary: Some("Standup".into()),
            starts_at: Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap()),
            ends_at: Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 30, 0).unwrap()),
            location: None,
            description: None,
            status: None,
            recurrence_rule: Some("FREQ=WEEKLY".into()),
            attendees: vec![],
            organizer: None,
        };
        let facts = vevent_to_facts(Some("Devansh"), &event, "standup@test");
        let primary = &facts[0];
        // A recurring event keeps surfacing on every occurrence, so its fact
        // must not expire after the first instance's DTEND.
        assert!(primary.valid_until.is_none());
        assert_eq!(primary.recurrence, RecurrenceType::Weekly);
    }
}
