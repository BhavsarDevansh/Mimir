//! CalDAV connector factory and supervisor round-trip tests.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{
    CalendarAuthMethod, CalendarConfigDto, CalendarConnectorFactory, ConnectorContext,
    ConnectorFactory, ConnectorRegistry, ConnectorSupervisor, SyncOptions,
};
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};

mod common;
use common::*;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wait_until_some_times_out_while_probe_is_pending() {
    let result = tokio::time::timeout(
        Duration::from_millis(200),
        tokio::spawn(async {
            wait_until_some(
                || async { std::future::pending::<Option<()>>().await },
                Duration::from_millis(50),
            )
            .await
        }),
    )
    .await;

    assert!(
        matches!(result, Ok(Err(join_error)) if join_error.is_panic()),
        "a pending probe must fail the helper deadline instead of hanging"
    );
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn factory_creates_connector_from_context_and_syncs() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("tok", &[("/cal/a.ics", ICAL_EVENT)], &[]),
    )
    .await;

    let store = store_with_app_password().await;
    let ctx = ConnectorContext::empty().with_secret_store(store);
    let connector = CalendarConnectorFactory::new()
        .create(app_password_config(&url), &ctx)
        .unwrap();
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("tok"));
}

#[tokio::test]
async fn factory_config_round_trips_through_serde() {
    let dto = CalendarConfigDto {
        calendar_url: "https://cal.example.com/x/".into(),
        auth: CalendarAuthMethod::AppPassword {
            username: "u@example.com".into(),
        },
        poll_interval_secs: 300,
        poll_jitter_secs: 10,
        display_name: Some("Work".into()),
    };
    let json = serde_json::to_value(&dto).unwrap();
    let back: CalendarConfigDto = serde_json::from_value(json).unwrap();
    assert_eq!(dto, back);
}
// ---------------------------------------------------------------------------
// Supervisor round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn supervisor_round_trips_and_persists_cursor() {
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/personal/", server.uri());
    // Health probe (Online) — mounted without a body constraint so every
    // cycle's PROPFIND succeeds.
    Mock::given(method("PROPFIND"))
        .and(path("/cal/personal/"))
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
    // sync-collection REPORT — matches any body so the runner's first
    // (full) auto-cycle and any subsequent cycle all return token-1 + event A.
    Mock::given(method("REPORT"))
        .and(path("/cal/personal/"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body("sup-token-1", &[("/cal/a.ics", ICAL_EVENT)], &[])),
        )
        .mount(&server)
        .await;

    let (kg, _db_dir) = init_kg().await;
    let store = store_with_app_password().await;
    // Long poll interval so only the runner's immediate first auto-cycle
    // runs during the test (no racing repeated cycles).
    let config = serde_json::to_string(&json!({
        "calendar_url": cal_url,
        "auth": { "kind": "app_password", "username": "devansh@example.com" },
        "poll_interval_secs": 3600,
        "poll_jitter_secs": 0,
        "__slug": "calendar-personal",
    }))
    .unwrap();
    let row = kg
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
    let (shutdown_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx)
        .with_secret_store(store);
    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // The runner's first cycle runs immediately and persists the cursor.
    wait_until_some(
        || async {
            kg.get_connector(row.id)
                .await
                .unwrap()
                .filter(|row| row.sync_cursor.as_deref() == Some("sup-token-1"))
        },
        Duration::from_secs(8),
    )
    .await;

    supervisor.shutdown().await;
    drop(shutdown_tx);
}
