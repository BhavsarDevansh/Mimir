//! CalDAV connector event extraction integration tests.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use chrono::{TimeZone, Utc};
use wiremock::MockServer;

use mimir_connectors::{Connector, SyncOptions};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};

mod common;
use common::*;

// ---------------------------------------------------------------------------
// C4 / #198: event → KB fact extraction + write-back
// ---------------------------------------------------------------------------

const ICAL_FULL: &str = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
PRODID:-//Mimir//Test//EN\n\
BEGIN:VEVENT\n\
UID:meet-1@test\n\
SUMMARY:Standup\n\
DTSTART;TZID=Europe/London:20250703T090000\n\
DTEND;TZID=Europe/London:20250703T093000\n\
LOCATION:Office\n\
ORGANIZER;CN=Devansh Bhavsar:mailto:devansh@example.com\n\
ATTENDEE;CN=Alice:mailto:alice@example.com\n\
ATTENDEE:mailto:bob@example.com\n\
RRULE:FREQ=WEEKLY\n\
STATUS:CONFIRMED\n\
END:VEVENT\n\
END:VCALENDAR";

#[tokio::test]
async fn extract_emits_event_location_attendee_facts_with_identity() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("t1", &[("/cal/meet-1.ics", ICAL_FULL)], &[]),
    )
    .await;

    let connector = make_connector_as(
        app_password_config(&url),
        store_with_app_password().await,
        None,
        "Devansh",
    );
    connector.sync(SyncOptions::default()).await.unwrap();
    let facts = connector.extract().await.unwrap();

    // Primary: user has_event <event> (Appointment, WEEKLY recurrence).
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.subject, "Devansh");
    assert_eq!(primary.subject_type, EntityType::Person);
    assert_eq!(primary.object, "Standup");
    assert_eq!(primary.object_type, Some(EntityType::Event));
    assert_eq!(primary.recurrence, RecurrenceType::Weekly);
    assert_eq!(primary.event_type, Some(EventType::Appointment));
    assert_eq!(primary.raw_reference.as_deref(), Some("meet-1@test"));
    // TZID Europe/London on 2025-07-03 09:00 BST → 08:00 UTC.
    assert_eq!(
        primary.valid_from,
        Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 0, 0).unwrap())
    );
    // Recurring (WEEKLY): the fact must not expire after the first instance's
    // DTEND, so valid_until is left unset (#248 review).
    assert_eq!(primary.valid_until, None);

    // Location: <event> located_in <place>.
    let loc = facts
        .iter()
        .find(|f| f.relationship_type == "located_in")
        .expect("location fact");
    assert_eq!(loc.subject, "Standup");
    assert_eq!(loc.subject_type, EntityType::Event);
    assert_eq!(loc.object, "Office");
    assert_eq!(loc.object_type, Some(EntityType::Place));
    assert_eq!(loc.event_type, None);
    // Secondary facts carry no temporal bounds, so they spawn no overlay.
    assert_eq!(loc.valid_from, None);
    assert_eq!(loc.valid_until, None);

    // Attendees: <person> attending <event> — one per attendee, by name/mail.
    let attendees: Vec<&_> = facts
        .iter()
        .filter(|f| f.relationship_type == "attending")
        .collect();
    assert_eq!(attendees.len(), 2, "two attendee facts");
    let attendee_subjects: Vec<&str> = attendees.iter().map(|f| f.subject.as_str()).collect();
    assert!(attendee_subjects.contains(&"Alice"));
    assert!(attendee_subjects.contains(&"bob@example.com"));
    for a in &attendees {
        assert_eq!(a.object, "Standup");
        assert_eq!(a.object_type, Some(EntityType::Event));
        assert_eq!(a.valid_from, None);
        assert_eq!(a.valid_until, None);
    }

    // No double-counting: exactly 1 primary + 1 location + 2 attendees.
    assert_eq!(facts.len(), 4);
}

#[tokio::test]
async fn extract_one_time_event_carries_dtend_as_valid_until() {
    // A non-recurring event keeps its DTEND as valid_until (the fact is a
    // bounded appointment), in contrast to recurring facts which leave it
    // unset (#248 review).
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("t1", &[("/cal/rome-1.ics", ICAL_EVENT)], &[]),
    )
    .await;

    let connector = make_connector_as(
        app_password_config(&url),
        store_with_app_password().await,
        None,
        "Devansh",
    );
    connector.sync(SyncOptions::default()).await.unwrap();
    let facts = connector.extract().await.unwrap();

    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.recurrence, RecurrenceType::None);
    assert_eq!(
        primary.valid_from,
        Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap())
    );
    assert_eq!(
        primary.valid_until,
        Some(Utc.with_ymd_and_hms(2025, 5, 7, 18, 0, 0).unwrap())
    );
}

#[tokio::test]
async fn extract_trims_padded_user_identity() {
    // A padded `[identity] name` is normalised to its trimmed value, so the
    // primary fact is authored against the canonical entity rather than a
    // duplicate "  Devansh  " person (#248 review).
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("t1", &[("/cal/meet-1.ics", ICAL_FULL)], &[]),
    )
    .await;

    let connector = make_connector_as(
        app_password_config(&url),
        store_with_app_password().await,
        None,
        "  Devansh  ",
    );
    connector.sync(SyncOptions::default()).await.unwrap();
    let facts = connector.extract().await.unwrap();
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.subject, "Devansh");
}

#[tokio::test]
async fn extract_without_identity_skips_primary_but_keeps_location_attendees() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("t1", &[("/cal/meet-1.ics", ICAL_FULL)], &[]),
    )
    .await;

    // No user identity injected.
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    connector.sync(SyncOptions::default()).await.unwrap();
    let facts = connector.extract().await.unwrap();
    assert!(facts.iter().all(|f| f.relationship_type != "has_event"));
    assert_eq!(facts.len(), 3, "location + 2 attendee facts only");
}
