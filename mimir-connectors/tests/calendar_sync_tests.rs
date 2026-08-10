//! CalDAV connector sync, OAuth refresh, and health integration tests.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{Connector, InMemorySecretStore, SecretBundle, SecretStore, SyncOptions};

mod common;
use common::*;

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
