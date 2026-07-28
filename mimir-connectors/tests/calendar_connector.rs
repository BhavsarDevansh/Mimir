//! Integration tests for the CalDAV calendar connector (Phase 3 C3 / #197):
//! app-password sync, incremental sync-token, OAuth token refresh, health
//! probing, factory construction, and a full supervisor round-trip.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{
    CalendarAuthMethod, CalendarConfigDto, CalendarConnector, CalendarConnectorFactory, Connector,
    ConnectorContext, ConnectorFactory, ConnectorRegistry, ConnectorSupervisor,
    InMemorySecretStore, SecretBundle, SecretStore, SupervisorConfig, SyncOptions,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};

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
        CalendarConnector::from_config_with_http(config, Some(store), cursor, None)
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
    // extract drains the buffer and (C3) emits no facts yet.
    let facts = connector.extract().await.unwrap();
    assert!(facts.is_empty(), "C3 emits no facts; C4 / #198 will");
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
            config_json: config,
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
