//! Shared fixtures for the connector integration tests: CalDAV calendar
//! fixtures (gated by the `calendar` feature) and the framework/supervisor
//! test harness used by the `test-mock-connector` supervisor tests.

#![allow(dead_code)]

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

#[cfg(feature = "calendar")]
use chrono::{Duration as ChronoDuration, Utc};
#[cfg(feature = "calendar")]
use mimir_connectors::{CalendarConnector, InMemorySecretStore, SecretBundle, SecretStore};
#[cfg(feature = "test-mock-connector")]
use mimir_connectors::{
    Connector, ConnectorError, FnConnectorFactory, MockConnector, MockConnectorFactory,
    MockSyncRecorder,
};
use mimir_connectors::{ConnectorRegistry, ConnectorSupervisor, SupervisorConfig};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};
#[cfg(feature = "calendar")]
use wiremock::matchers::{body_string_contains, method, path};
#[cfg(feature = "calendar")]
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// CalDAV calendar fixtures + helpers (gated by the `calendar` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "calendar")]
pub const ICAL_EVENT: &str = "BEGIN:VCALENDAR\n\
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

#[cfg(feature = "calendar")]
pub const ICAL_RECURRING: &str = "BEGIN:VCALENDAR\n\
VERSION:2.0\n\
BEGIN:VEVENT\n\
UID:bday@test\n\
SUMMARY:Mom's birthday\n\
DTSTART:20250101T090000Z\n\
RRULE:FREQ=YEARLY\n\
END:VEVENT\n\
END:VCALENDAR";

#[cfg(feature = "calendar")]
pub fn sync_body(token: &str, items: &[(&str, &str)], deleted: &[&str]) -> String {
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

#[cfg(feature = "calendar")]
pub async fn mount_sync(
    server: &MockServer,
    path_suffix: &str,
    token_req: Option<&str>,
    body: String,
) {
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

#[cfg(feature = "calendar")]
pub fn app_password_config(calendar_url: &str) -> serde_json::Value {
    json!({
        "calendar_url": calendar_url,
        "auth": { "kind": "app_password", "username": "devansh@example.com" },
        "poll_interval_secs": 1,
        "poll_jitter_secs": 0,
        "__slug": "calendar-personal",
    })
}

#[cfg(feature = "calendar")]
pub fn oauth_config(calendar_url: &str, token_endpoint: &str) -> serde_json::Value {
    json!({
        "calendar_url": calendar_url,
        "auth": {
            "kind": "oauth",
            "auth_uri": "https://oauth.example.com/authorize",
            "token_endpoint": token_endpoint,
            "client_id": "mimir-client",
            "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
        },
        "poll_interval_secs": 1,
        "poll_jitter_secs": 0,
        "__slug": "calendar-google",
    })
}

#[cfg(feature = "calendar")]
pub async fn store_with_app_password() -> Arc<dyn SecretStore> {
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

#[cfg(feature = "calendar")]
pub async fn store_with_expired_oauth(refresh_token: &str) -> Arc<dyn SecretStore> {
    let store = Arc::new(InMemorySecretStore::new());
    store
        .store(
            "calendar-google",
            &SecretBundle::OAuth {
                access_token: "stale-token".into(),
                refresh_token: Some(refresh_token.to_string()),
                expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
                client_secret: None,
            },
        )
        .await
        .unwrap();
    store
}

#[cfg(feature = "calendar")]
pub fn make_connector(
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
#[cfg(feature = "calendar")]
pub fn make_connector_as(
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

pub async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

pub fn fast_config() -> SupervisorConfig {
    SupervisorConfig {
        max_failures: 5,
        base_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

// ---------------------------------------------------------------------------
// Knowledge-graph test harness
// ---------------------------------------------------------------------------

pub fn upsert(
    slug: &str,
    ctype: ConnectorType,
    backend: &str,
    status: ConnectorStatus,
) -> UpsertConnectorInput {
    UpsertConnectorInput {
        connector_type: ctype,
        slug: slug.to_string(),
        backend: backend.to_string(),
        display_name: slug.to_string(),
        config_json: "{}".to_string(),
        status: Some(status),
        auth_state: Some(ConnectorAuthState::Authenticated),
    }
}

pub fn make_supervisor(
    kg: Arc<KnowledgeGraph>,
    registry: Arc<ConnectorRegistry>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> ConnectorSupervisor {
    ConnectorSupervisor::new(
        registry,
        kg,
        SupervisorConfig {
            max_failures: 3,
            base_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(40),
        },
        shutdown,
    )
}

/// Poll an async `predicate` until it returns true or `timeout` elapses.
pub async fn wait_for_async<F, Fut>(predicate: F, timeout: Duration)
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("wait_for_async timed out after {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub fn with_slug(slug: &str, extra: serde_json::Value) -> String {
    let mut cfg = extra;
    if let serde_json::Value::Object(map) = &mut cfg {
        map.insert("__slug".to_string(), json!(slug));
    }
    serde_json::to_string(&cfg).unwrap()
}

// ---------------------------------------------------------------------------
// Registries (MockConnector-backed)
// ---------------------------------------------------------------------------

/// Build a registry whose `MockConnectorFactory` reads behaviour entirely from
/// `config_json` (including the row slug/type, smuggled in by the supervisor at
/// restore time). The mock is the single connector used by every lifecycle
/// test.
#[cfg(feature = "test-mock-connector")]
pub fn test_registry() -> Arc<ConnectorRegistry> {
    let registry = ConnectorRegistry::new();
    for ctype in [
        ConnectorType::Gmail,
        ConnectorType::Calendar,
        ConnectorType::Photos,
    ] {
        registry
            .register(ctype, "test".to_string(), MockConnectorFactory)
            .unwrap();
    }
    Arc::new(registry)
}

/// A registry whose factory injects a shared [`MockSyncRecorder`] into every
/// constructed `MockConnector`, so the F9 trigger tests can observe the
/// `SyncOptions` each `sync()` receives and the peak concurrency. The recorder
/// is attached via [`MockConnector::with_recorder`] (not the config path).
#[cfg(feature = "test-mock-connector")]
pub fn recording_registry(recorder: Arc<MockSyncRecorder>) -> Arc<ConnectorRegistry> {
    let registry = ConnectorRegistry::new();
    for ctype in [
        ConnectorType::Gmail,
        ConnectorType::Calendar,
        ConnectorType::Photos,
    ] {
        let rec = recorder.clone();
        let factory = FnConnectorFactory::new(
            move |config, _ctx| -> Result<Arc<dyn Connector>, ConnectorError> {
                Ok(
                    Arc::new(MockConnector::from_config(config)?.with_recorder(rec.clone()))
                        as Arc<dyn Connector>,
                )
            },
        );
        registry
            .register(ctype, "test".to_string(), factory)
            .unwrap();
    }
    Arc::new(registry)
}
