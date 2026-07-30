//! Integration tests for the CalDAV calendar connector (Phase 3 C3 / #197):
//! app-password sync, incremental sync-token, OAuth token refresh, health
//! probing, factory construction, and a full supervisor round-trip.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{
    CalendarAuthMethod, CalendarConfigDto, CalendarConnector, CalendarConnectorFactory, Connector,
    ConnectorAction, ConnectorContext, ConnectorError, ConnectorFactory, ConnectorRegistry,
    ConnectorSupervisor, InMemorySecretStore, SecretBundle, SecretStore, SupervisorConfig,
    SyncOptions,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{
    ConnectorAuthState, ConnectorStatus, ConnectorType, EventType, RecurrenceType,
};

// ---------------------------------------------------------------------------
// Fixtures + helpers
// ---------------------------------------------------------------------------

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

const ICAL_RECURRING: &str = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
BEGIN:VEVENT\n\
UID:bday@test\n\
SUMMARY:Mom's birthday\n\
DTSTART:20250101T090000Z\n\
RRULE:FREQ=YEARLY\n\
END:VEVENT\n\
END:VCALENDAR";

fn sync_body(token: &str, items: &[(&str, &str)], deleted: &[&str]) -> String {
    let mut responses = String::new();
    for (href, ical) in items {
        responses.push_str(&format!(
            "<d:response><d:href>{href}</d:href><d:propstat><d:prop>\
<cal:calendar-data><![CDATA[{ical}]]></cal:calendar-data></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
        ));
    }
    for href in deleted {
        responses.push_str(&format!(
            "<d:response><d:href>{href}</d:href><d:propstat><d:prop/>\
<d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>"
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\n\
<d:sync-token>{token}</d:sync-token>\n{responses}\n</d:multistatus>"
    )
}

async fn mount_sync(server: &MockServer, path_suffix: &str, token_req: Option<&str>, body: String) {
    let matcher = body_string_contains(
        token_req
            .map(|t| format!("<d:sync-token>{t}</d:sync-token>"))
            .unwrap_or_else(|| "<d:sync-token/>".to_string()),
    );
    Mock::given(method("REPORT"))
        .and(path(path_suffix))
        .and(matcher)
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(body),
        )
        .up_to_n_times(1)
        .mount(server)
        .await;
}

fn app_password_config(calendar_url: &str) -> serde_json::Value {
    json!({
        "calendar_url": calendar_url,
        "auth": { "kind": "app_password", "username": "devansh@example.com" },
        "poll_interval_secs": 1,
        "poll_jitter_secs": 0,
        "__slug": "calendar-personal",
    })
}

fn oauth_config(calendar_url: &str, token_endpoint: &str) -> serde_json::Value {
    json!({
        "calendar_url": calendar_url,
        "auth": {
            "kind": "oauth",
            "token_endpoint": token_endpoint,
            "client_id": "mimir-client",
            "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
        },
        "poll_interval_secs": 1,
        "poll_jitter_secs": 0,
        "__slug": "calendar-google",
    })
}

async fn store_with_app_password() -> Arc<dyn SecretStore> {
    let store = Arc::new(InMemorySecretStore::new());
    store
        .store(
            "calendar-personal",
            &SecretBundle::AppPassword {
                password: "app-pass".into(),
            },
        )
        .await
        .unwrap();
    store
}

async fn store_with_expired_oauth(refresh_token: &str) -> Arc<dyn SecretStore> {
    let store = Arc::new(InMemorySecretStore::new());
    store
        .store(
            "calendar-google",
            &SecretBundle::OAuth {
                access_token: "stale-token".into(),
                refresh_token: Some(refresh_token.to_string()),
                expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
            },
        )
        .await
        .unwrap();
    store
}

fn make_connector(
    config: serde_json::Value,
    store: Arc<dyn SecretStore>,
    cursor: Option<String>,
) -> Arc<CalendarConnector> {
    Arc::new(
        CalendarConnector::from_config_with_http(config, Some(store), None, cursor, None)
            .expect("connector constructs"),
    )
}

/// Like [`make_connector`] but injects a canonical user identity so the
/// extractor authors `user has_event <event>` (and the event surfaces in the
/// user's "Upcoming" section).
fn make_connector_as(
    config: serde_json::Value,
    store: Arc<dyn SecretStore>,
    cursor: Option<String>,
    user_identity: &str,
) -> Arc<CalendarConnector> {
    Arc::new(
        CalendarConnector::from_config_with_http(
            config,
            Some(store),
            Some(user_identity.to_string()),
            cursor,
            None,
        )
        .expect("connector constructs"),
    )
}

// ---------------------------------------------------------------------------
// App-password sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn app_password_sync_stages_events_and_returns_cursor() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("token-2", &[("/cal/uid-1.ics", ICAL_EVENT)], &[]),
    )
    .await;

    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1, "one VEVENT staged");
    assert_eq!(outcome.new_cursor.as_deref(), Some("token-2"));
    // extract drains the buffer into C4 facts. Without a user identity the
    // primary `has_event` fact is skipped, so only the event→location fact
    // is emitted (ICAL_EVENT has a LOCATION, no attendees).
    let facts = connector.extract().await.unwrap();
    assert_eq!(facts.len(), 1, "one location fact for the staged event");
    assert_eq!(facts[0].relationship_type, "located_in");
    assert_eq!(facts[0].subject, "Trip to Rome");
    assert_eq!(facts[0].object, "Rome");
}

// ---------------------------------------------------------------------------
// Incremental sync-token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn incremental_sync_uses_persisted_sync_token() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    // Full sync → token-1, event A.
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("token-1", &[("/cal/a.ics", ICAL_EVENT)], &[]),
    )
    .await;
    // Incremental (token-1) → token-2, event B, deleted A.
    mount_sync(
        &server,
        "/cal/personal/",
        Some("token-1"),
        sync_body(
            "token-2",
            &[("/cal/b.ics", ICAL_RECURRING)],
            &["/cal/a.ics"],
        ),
    )
    .await;

    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    let first = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(first.fetched, 1);
    assert_eq!(first.new_cursor.as_deref(), Some("token-1"));
    connector.extract().await.unwrap(); // drain

    // Second (non-full) sync must send token-1; the incremental mock only
    // matches a body containing `<d:sync-token>token-1</d:sync-token>`.
    let second = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(second.fetched, 1, "only the changed event B is fetched");
    assert_eq!(second.new_cursor.as_deref(), Some("token-2"));
}

#[tokio::test]
async fn full_sync_ignores_cursor() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    mount_sync(
        &server,
        "/cal/personal/",
        None,
        sync_body("token-fresh", &[("/cal/a.ics", ICAL_EVENT)], &[]),
    )
    .await;
    // A stale cursor is present but `full` must re-fetch from scratch.
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        Some("token-stale".to_string()),
    );
    let outcome = connector
        .sync(SyncOptions {
            full: true,
            since: None,
        })
        .await
        .unwrap();
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("token-fresh"));
}

// ---------------------------------------------------------------------------
// OAuth refresh
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth_refreshes_expired_token_then_syncs() {
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/google/", server.uri());
    let token_url = format!("{}/oauth/token", server.uri());

    // Token endpoint: returns a fresh access token + rotated refresh token.
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=rt-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-token",
            "refresh_token": "rt-2",
            "expires_in": 3600,
            "token_type": "Bearer",
        })))
        .mount(&server)
        .await;

    // CalDAV sync only succeeds with the fresh bearer token.
    Mock::given(method("REPORT"))
        .and(path("/cal/google/"))
        .and(header("authorization", "Bearer fresh-token"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body("gtoken-1", &[("/cal/g.ics", ICAL_EVENT)], &[])),
        )
        .mount(&server)
        .await;

    let store = store_with_expired_oauth("rt-1").await;
    let connector = make_connector(oauth_config(&cal_url, &token_url), store.clone(), None);
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("gtoken-1"));

    // The refreshed bundle is persisted back to the store.
    let bundle = store.load("calendar-google").await.unwrap().unwrap();
    let SecretBundle::OAuth {
        access_token,
        refresh_token,
        expires_at,
    } = bundle
    else {
        panic!("expected OAuth bundle, got {bundle:?}");
    };
    assert_eq!(access_token, "fresh-token");
    assert_eq!(refresh_token.as_deref(), Some("rt-2"));
    assert!(expires_at.is_some(), "expiry derived from expires_in");
}

#[tokio::test]
async fn oauth_refresh_without_refresh_token_in_response_retains_prior() {
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/google/", server.uri());
    let token_url = format!("{}/oauth/token", server.uri());

    // Token endpoint returns a fresh access token but omits refresh_token —
    // the connector must retain the prior refresh token (PR #242 review #14).
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-token",
            "expires_in": 3600,
            "token_type": "Bearer",
        })))
        .mount(&server)
        .await;

    Mock::given(method("REPORT"))
        .and(path("/cal/google/"))
        .and(header("authorization", "Bearer fresh-token"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body("gtoken-1", &[("/cal/g.ics", ICAL_EVENT)], &[])),
        )
        .mount(&server)
        .await;

    let store = store_with_expired_oauth("rt-1").await;
    let connector = make_connector(oauth_config(&cal_url, &token_url), store.clone(), None);
    connector.sync(SyncOptions::default()).await.unwrap();

    // The persisted bundle must still carry the original refresh token.
    let bundle = store.load("calendar-google").await.unwrap().unwrap();
    let SecretBundle::OAuth { refresh_token, .. } = bundle else {
        panic!("expected OAuth bundle, got {bundle:?}");
    };
    assert_eq!(
        refresh_token.as_deref(),
        Some("rt-1"),
        "prior refresh token must be retained when the response omits one"
    );
}

#[tokio::test]
async fn oauth_unknown_expiry_does_not_force_refresh_on_every_cycle() {
    let server = MockServer::start().await;
    let cal_url = format!("{}/cal/google/", server.uri());
    let token_url = format!("{}/oauth/token", server.uri());

    // A valid (non-expired) access token with an *unknown* expiry must be
    // reused as-is — no refresh POST should reach the token endpoint
    // (PR #242 review #11). The mock server mounts no POST handler, so a
    // refresh attempt would fail with a wiremock "no matching mock" error.
    let store = Arc::new(InMemorySecretStore::new());
    store
        .store(
            "calendar-google",
            &SecretBundle::OAuth {
                access_token: "live-token".into(),
                refresh_token: Some("rt-1".into()),
                expires_at: None,
            },
        )
        .await
        .unwrap();

    Mock::given(method("REPORT"))
        .and(path("/cal/google/"))
        .and(header("authorization", "Bearer live-token"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body("gtoken-1", &[("/cal/g.ics", ICAL_EVENT)], &[])),
        )
        .mount(&server)
        .await;

    let connector = make_connector(oauth_config(&cal_url, &token_url), store.clone(), None);
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("gtoken-1"));
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_reports_online_for_valid_app_password() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
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
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    use mimir_connectors::HealthStatus;
    assert_eq!(connector.health().await.unwrap(), HealthStatus::Online);
}

#[tokio::test]
async fn health_reports_not_configured_without_secret() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new()); // empty
    let connector = make_connector(app_password_config(&url), store, None);
    use mimir_connectors::HealthStatus;
    assert_eq!(
        connector.health().await.unwrap(),
        HealthStatus::NotConfigured
    );
}

#[tokio::test]
async fn health_reports_auth_expired_on_401() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    use mimir_connectors::HealthStatus;
    assert_eq!(connector.health().await.unwrap(), HealthStatus::AuthExpired);
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

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn fast_config() -> SupervisorConfig {
    SupervisorConfig {
        max_failures: 5,
        base_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let row = kg.get_connector(row.id).await.unwrap().unwrap();
        if row.sync_cursor.as_deref() == Some("sup-token-1") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sync-token cursor never persisted"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    supervisor.shutdown().await;
    drop(shutdown_tx);
}

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

#[tokio::test]
async fn write_back_creates_updates_and_deletes_events() {
    use wiremock::matchers::{body_string_contains, header, method, path};

    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());

    // create_event: PUT with If-None-Match: *, respond 201 + ETag.
    Mock::given(method("PUT"))
        .and(path("/cal/personal/new-1.ics"))
        .and(header("if-none-match", "*"))
        .and(body_string_contains("SUMMARY:Dentist"))
        .and(body_string_contains("DTSTART:20250901T090000Z"))
        .and(body_string_contains("ATTENDEE:mailto:alice@example.com"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("etag", "\"v1\"")
                .set_body_string(""),
        )
        .mount(&server)
        .await;
    // update_event: PUT with If-Match: "v1", respond 200 + new ETag.
    Mock::given(method("PUT"))
        .and(path("/cal/personal/new-1.ics"))
        .and(header("if-match", "\"v1\""))
        .and(body_string_contains("SUMMARY:Dentist (moved)"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v2\"")
                .set_body_string(""),
        )
        .mount(&server)
        .await;
    // delete_event: DELETE with If-Match: "v2", respond 204.
    Mock::given(method("DELETE"))
        .and(path("/cal/personal/new-1.ics"))
        .and(header("if-match", "\"v2\""))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );

    let created = connector
        .act(ConnectorAction {
            kind: "create_event".to_string(),
            payload: json!({
                "uid": "new-1",
                "href": format!("{url}new-1.ics"),
                "summary": "Dentist",
                "start": "2025-09-01T09:00:00Z",
                "end": "2025-09-01T09:30:00Z",
                "location": "Surgery",
                "attendees": ["alice@example.com"],
            }),
        })
        .await
        .unwrap();
    assert!(created.success);
    assert_eq!(created.message.as_deref(), Some("\"v1\""));

    let updated = connector
        .act(ConnectorAction {
            kind: "update_event".to_string(),
            payload: json!({
                "uid": "new-1",
                "href": format!("{url}new-1.ics"),
                "etag": "\"v1\"",
                "summary": "Dentist (moved)",
                "start": "2025-09-01T10:00:00Z",
                "end": "2025-09-01T10:30:00Z",
            }),
        })
        .await
        .unwrap();
    assert!(updated.success);
    assert_eq!(updated.message.as_deref(), Some("\"v2\""));

    let deleted = connector
        .act(ConnectorAction {
            kind: "delete_event".to_string(),
            payload: json!({
                "href": format!("{url}new-1.ics"),
                "etag": "\"v2\"",
            }),
        })
        .await
        .unwrap();
    assert!(deleted.success);
}

#[tokio::test]
async fn write_back_delete_is_idempotent_on_404() {
    use wiremock::matchers::{method, path};

    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    Mock::given(method("DELETE"))
        .and(path("/cal/personal/gone.ics"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    let res = connector
        .act(ConnectorAction {
            kind: "delete_event".to_string(),
            payload: json!({ "href": format!("{url}gone.ics") }),
        })
        .await
        .unwrap();
    assert!(res.success, "a 404 delete is idempotent success");
}

#[tokio::test]
async fn write_back_unsupported_action_errors() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );
    let err = connector
        .act(ConnectorAction {
            kind: "bogus".to_string(),
            payload: json!({}),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::UnsupportedAction(_)));
}

// ---------------------------------------------------------------------------
// C4 / #198: end-to-end sync → KB → events-subsystem "Upcoming"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_back_rejects_href_outside_calendar_origin() {
    // A caller-supplied href pointing at another host must be rejected
    // before any request is issued, so the stored credentials are never
    // sent there (#248 review).
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );

    let err = connector
        .act(ConnectorAction {
            kind: "delete_event".to_string(),
            payload: json!({ "href": "http://evil.example.com/cal/personal/x.ics" }),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
}

#[tokio::test]
async fn write_back_rejects_href_with_wrong_path_on_same_origin() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    let connector = make_connector(
        app_password_config(&url),
        store_with_app_password().await,
        None,
    );

    let err = connector
        .act(ConnectorAction {
            kind: "delete_event".to_string(),
            payload: json!({ "href": format!("{}../other/x.ics", url) }),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
}

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
