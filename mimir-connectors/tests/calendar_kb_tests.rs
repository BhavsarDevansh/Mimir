//! CalDAV connector end-to-end knowledge-graph integration tests.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{CalendarConnectorFactory, ConnectorRegistry, ConnectorSupervisor};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{
    ConnectorAuthState, ConnectorStatus, ConnectorType, EventType,
};

mod common;
use common::*;

#[tokio::test]
async fn calendar_sync_surfaces_upcoming_event_for_user() {
    let (kg, _dir) = init_kg().await;
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/personal/", server.uri());

    // Health probe (Online) — PROPFIND resourcetype for every cycle.
    Mock::given(wiremock::matchers::method("PROPFIND"))
        .and(wiremock::matchers::path("/cal/personal/"))
        .respond_with(
            ResponseTemplate::new(207).set_body_string(
                "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\
<d:response><d:href>/cal/personal/</d:href><d:propstat><d:prop>\
<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
            ),
        )
        .mount(&server)
        .await;

    // A future-dated one-time event (now + 5 days) so it lands inside the
    // 30-day Upcoming horizon.
    let start = Utc::now() + ChronoDuration::days(5);
    let start_ical = start.format("%Y%m%dT%H%M%SZ").to_string();
    let ical = format!(
        "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\n\
UID:future-1@test\nSUMMARY:Conference\n\
DTSTART:{start_ical}\nLOCATION:London\nEND:VEVENT\nEND:VCALENDAR"
    );
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("upc-1", &[("/cal/future-1.ics", &ical)], &[]),
    )
    .await;

    let config = app_password_config(&cal_url);
    let _row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Calendar,
            slug: "calendar-personal".to_string(),
            backend: "caldav".to_string(),
            display_name: "Calendar".to_string(),
            config_json: config.to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Calendar, "caldav", CalendarConnectorFactory)
        .unwrap();
    let (_shutdown_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx)
        .with_secret_store(store_with_app_password().await)
        .with_user_identity("Devansh");

    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the user entity + its `has_event` fact to land.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let devansh = entity_id(&kg, "Devansh").await;
        if let Some(uid) = devansh {
            if !kg.get_facts_by_subject(uid, 100).await.unwrap().is_empty() {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "event fact never landed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let devansh = entity_id(&kg, "Devansh").await.expect("Devansh entity");
    let upcoming = kg.render_upcoming_section(devansh, 30, 10).await.unwrap();
    assert!(
        upcoming.contains("Conference"),
        "future event surfaces in Upcoming: {upcoming}"
    );

    // The event overlay is an Appointment (not a Reminder/Task).
    let fact = kg
        .get_facts_by_subject(devansh, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.object_literal.is_none())
        .expect("has_event fact");
    let event = kg
        .get_event_by_fact(fact.id)
        .await
        .unwrap()
        .expect("overlay");
    assert_eq!(event.event_type(), Some(EventType::Appointment));

    // Secondary facts (location/attendance) must not spawn their own
    // events-subsystem overlays — only the primary `has_event` fact drives
    // one (#198 review). The `located_in` fact (Event → Place) is the only
    // fact whose subject is the Conference event entity, so assert none of
    // its facts carry an overlay.
    let conference = entity_id(&kg, "Conference")
        .await
        .expect("Conference entity");
    for fact in kg.get_facts_by_subject(conference, 100).await.unwrap() {
        assert!(
            kg.get_event_by_fact(fact.id).await.unwrap().is_none(),
            "secondary fact {} must not spawn an event overlay",
            fact.id,
        );
    }

    supervisor.shutdown().await;
}

async fn entity_id(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
    let results = kg.search_entities(name, 10).await.unwrap();
    results.into_iter().next().map(|r| r.entity.id)
}
