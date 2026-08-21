//! iCalendar parsing and fact-extraction tests.

use super::*;
use crate::ical::parse::parse_ical_datetime;
use chrono::TimeZone as _;
use chrono::Utc;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};

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

/// Every predicate the iCalendar extractor emits must be part of the canonical
/// vocabulary (`mimir_knowledge::is_canonical_predicate_name`), which the
/// knowledge crate pins to the migration seed (issue #412). A connector
/// cannot silently auto-create a `relationship_types` row on first sync.
#[test]
fn emitted_predicates_are_registered_connector_vocabulary() {
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
        organizer: None,
    };
    let facts = vevent_to_facts(Some("Devansh"), &event, "raw-1");
    assert!(
        facts.len() >= 3,
        "primary + location + attendees expected: {facts:?}"
    );
    for fact in facts {
        assert!(
            mimir_knowledge::is_canonical_predicate_name(&fact.relationship_type),
            "calendar-emitted predicate {} must be canonical vocabulary",
            fact.relationship_type
        );
    }
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
