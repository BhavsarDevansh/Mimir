//! Unit tests for the Microsoft Graph calendar backend (issue #474):
//! transport (delta sync, paging, tombstones, origin checks), the
//! connector cycle (sync → extract → deletions), credential refresh, and
//! the wizard-facing config surface.

use std::sync::Arc;

use chrono::{TimeZone as _, Utc};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::calendar::graph::client::{GRAPH_BASE_URL, GraphClient};
use crate::connector::{Connector, ConnectorError, SyncOptions};
use crate::secrets::{InMemorySecretStore, SecretBundle, SecretStore};
use mimir_knowledge::models::enums::{ConnectorAuthState, RecurrenceType};

fn http_client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

fn oauth_config(base_url: &str, token_endpoint: &str) -> serde_json::Value {
    serde_json::json!({
        "auth": {
            "kind": "oauth",
            "auth_uri": "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "token_endpoint": token_endpoint,
            "client_id": "cid",
            "scopes": ["https://graph.microsoft.com/Calendars.Read", "offline_access"],
        },
        "base_url": base_url,
    })
}

fn oauth_bundle(access_token: &str, expires_at: Option<chrono::DateTime<Utc>>) -> SecretBundle {
    SecretBundle::OAuth {
        access_token: access_token.into(),
        refresh_token: Some("rt".into()),
        expires_at,
        client_secret: None,
    }
}

fn event_json(id: &str, subject: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "subject": subject,
        "start": {"dateTime": "2025-05-03T09:00:00", "timeZone": "UTC"},
        "end": {"dateTime": "2025-05-07T18:00:00", "timeZone": "UTC"},
        "location": {"displayName": "Rome"},
        "attendees": [
            {"emailAddress": {"name": "Alice", "address": "alice@example.com"}, "type": "required"}
        ],
        "recurrence": {"pattern": {"type": "weekly", "interval": 1}, "range": {"type": "noEnd"}},
        "isCancelled": false
    })
}

fn delta_body(events: &[serde_json::Value], delta_link: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('me')/events",
        "value": events,
        "@odata.deltaLink": delta_link,
    })
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_events_full_returns_events_and_delta_link() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let delta_link = format!("{base}/me/events/delta?$deltatoken=token-2");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(header("authorization", "Bearer t0ken"))
        .and(header("prefer", "outlook.timezone=\"UTC\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-1", "Trip to Rome")],
            Some(&delta_link),
        )))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let res = client.sync_events(None).await.unwrap();
    assert_eq!(res.events.len(), 1);
    assert_eq!(res.events[0].id, "evt-1");
    assert_eq!(res.events[0].subject.as_deref(), Some("Trip to Rome"));
    assert!(res.deleted.is_empty());
    assert_eq!(res.new_delta_link.as_deref(), Some(delta_link.as_str()));
}

#[tokio::test]
async fn sync_events_incremental_requests_stored_delta_link() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let delta_link = format!("{base}/me/events/delta?$deltatoken=token-1");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$deltatoken", "token-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(&[], Some(&delta_link))))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let res = client.sync_events(Some(&delta_link)).await.unwrap();
    assert!(res.events.is_empty());
    assert_eq!(res.new_delta_link.as_deref(), Some(delta_link.as_str()));
}

#[tokio::test]
async fn sync_events_gone_resets_to_full_sync() {
    // A stored delta token can expire or be invalidated by a server-side
    // reset; the Graph delta contract answers `410 Gone` and the client
    // must restart with a full synchronization (so a stale cursor
    // self-heals instead of failing every cycle).
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let expired = format!("{base}/me/events/delta?$deltatoken=expired");
    let fresh = format!("{base}/me/events/delta?$deltatoken=fresh");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$deltatoken", "expired"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-1", "Trip to Rome")],
            Some(&fresh),
        )))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let res = client.sync_events(Some(&expired)).await.unwrap();
    // The full re-sync must re-fetch the whole event set and yield the
    // fresh cursor.
    assert_eq!(res.events.len(), 1);
    assert_eq!(res.events[0].id, "evt-1");
    assert_eq!(res.new_delta_link.as_deref(), Some(fresh.as_str()));
}

#[tokio::test]
async fn sync_events_sync_state_not_found_resets_to_full_sync() {
    // Some services surface an expired delta token as a `400` whose body
    // carries the `syncStateNotFound` error code — the same reset contract
    // as `410 Gone`.
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let expired = format!("{base}/me/events/delta?$deltatoken=expired");
    let fresh = format!("{base}/me/events/delta?$deltatoken=fresh");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$deltatoken", "expired"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {"code": "syncStateNotFound", "message": "Token is not valid."}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-1", "Trip to Rome")],
            Some(&fresh),
        )))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let res = client.sync_events(Some(&expired)).await.unwrap();
    assert_eq!(res.events.len(), 1);
    assert_eq!(res.new_delta_link.as_deref(), Some(fresh.as_str()));
}

#[tokio::test]
async fn sync_events_plain_400_is_not_silently_reset() {
    // Only the documented reset signals (`410`, or `400` with the
    // `syncStateNotFound` code) restart the sync; any other failure still
    // surfaces as an error so a real server problem is not masked.
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let expired = format!("{base}/me/events/delta?$deltatoken=expired");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {"code": "invalidRequest", "message": "nope"}
        })))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let err = client.sync_events(Some(&expired)).await.unwrap_err();
    assert!(
        matches!(err, ConnectorError::Other(_)),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn sync_events_pages_through_next_link() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let next = format!("{base}/me/events/delta?$skiptoken=page-2");
    let delta_link = format!("{base}/me/events/delta?$deltatoken=final");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$deltatoken", "token-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": [event_json("evt-1", "First")],
            "@odata.nextLink": next,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$skiptoken", "page-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-2", "Second")],
            Some(&delta_link),
        )))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base.clone(), "t0ken".into());
    let res = client
        .sync_events(Some(&format!("{base}/me/events/delta?$deltatoken=token-1")))
        .await
        .unwrap();
    assert_eq!(res.events.len(), 2);
    assert_eq!(res.events[0].id, "evt-1");
    assert_eq!(res.events[1].id, "evt-2");
    assert_eq!(res.new_delta_link.as_deref(), Some(delta_link.as_str()));
}

#[tokio::test]
async fn sync_events_reports_removed_as_deleted() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let mut removed = event_json("evt-gone", "Gone");
    removed["@removed"] = serde_json::json!({"reason": "deleted"});
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(&[removed], None)))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let res = client.sync_events(None).await.unwrap();
    assert!(res.events.is_empty());
    assert_eq!(res.deleted, vec!["evt-gone"]);
}

#[tokio::test]
async fn sync_events_401_maps_to_not_authenticated() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = GraphClient::new(http_client(), base, "t0ken".into());
    assert!(matches!(
        client.sync_events(None).await,
        Err(ConnectorError::NotAuthenticated)
    ));
}

#[tokio::test]
async fn sync_events_rejects_foreign_delta_link() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let client = GraphClient::new(http_client(), base, "t0ken".into());
    let err = client
        .sync_events(Some(
            "https://evil.example.com/me/events/delta?$deltatoken=x",
        ))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("outside the configured service origin"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn probe_ok_verifies_scope() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events"))
        .and(query_param("$top", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "value": [] })))
        .mount(&server)
        .await;
    let client = GraphClient::new(http_client(), base, "t0ken".into());
    assert!(client.probe().await.is_ok());
}

#[tokio::test]
async fn probe_401_maps_to_not_authenticated() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let client = GraphClient::new(http_client(), base, "t0ken".into());
    assert!(matches!(
        client.probe().await,
        Err(ConnectorError::NotAuthenticated)
    ));
}

// ---------------------------------------------------------------------------
// Connector cycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_extract_produces_fact_cluster() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let delta_link = format!("{base}/me/events/delta?$deltatoken=token-2");
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-1", "Trip to Rome")],
            Some(&delta_link),
        )))
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    store
        .store("calendar", &oauth_bundle("t0ken", None))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, "https://oauth.example.com/token"),
        Some(store),
        Some("Devansh".to_string()),
        None,
        Some(http_client()),
    )
    .unwrap();

    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some(delta_link.as_str()));

    let facts = connector.extract().await.unwrap();
    // 1 primary has_event + 1 located_in + 1 attending = 3.
    assert_eq!(facts.len(), 3);
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.subject, "Devansh");
    assert_eq!(primary.object, "Trip to Rome");
    assert_eq!(primary.recurrence, RecurrenceType::Weekly);
    assert_eq!(primary.raw_reference.as_deref(), Some("evt-1"));
    assert_eq!(
        primary.valid_from,
        Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap())
    );
    let loc = facts
        .iter()
        .find(|f| f.relationship_type == "located_in")
        .expect("located_in fact");
    assert_eq!(loc.subject, "Trip to Rome");
    assert_eq!(loc.object, "Rome");
    let attending = facts
        .iter()
        .find(|f| f.relationship_type == "attending")
        .expect("attending fact");
    assert_eq!(attending.subject, "Alice");
    assert_eq!(attending.object, "Trip to Rome");
}

#[tokio::test]
async fn sync_stages_tombstones_and_extract_deletions_reports_them() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let mut removed = event_json("evt-gone", "Gone");
    removed["@removed"] = serde_json::json!({"reason": "deleted"});
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(&[removed], None)))
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    store
        .store("calendar", &oauth_bundle("t0ken", None))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, "https://oauth.example.com/token"),
        Some(store),
        None,
        None,
        Some(http_client()),
    )
    .unwrap();

    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 0);
    let deletions = connector.extract_deletions().await.unwrap();
    assert_eq!(deletions, vec!["evt-gone"]);
    connector.acknowledge_deletions(&deletions).await.unwrap();
    assert!(connector.extract_deletions().await.unwrap().is_empty());
}

#[tokio::test]
async fn repeated_staging_deduplicates_events_by_id() {
    // A cycle cancelled after `sync` staged events but before `extract`
    // drained the buffer reuses the previous cursor and stages the same delta
    // again (issue #314); the buffer must keep one entry per event id so
    // `extract` authors one fact cluster per event, not one per staged copy.
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(delta_body(&[event_json("evt-1", "Trip to Rome")], None)),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    store
        .store("calendar", &oauth_bundle("t0ken", None))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, "https://oauth.example.com/token"),
        Some(store),
        Some("Devansh".to_string()),
        None,
        Some(http_client()),
    )
    .unwrap();

    // Stage the same delta twice without draining the buffer in between.
    assert_eq!(
        connector
            .sync(SyncOptions::default())
            .await
            .unwrap()
            .fetched,
        1
    );
    assert_eq!(
        connector
            .sync(SyncOptions::default())
            .await
            .unwrap()
            .fetched,
        1
    );
    let facts = connector.extract().await.unwrap();
    // One cluster per event id: 1 primary has_event + 1 located_in + 1
    // attending = 3, never 6.
    assert_eq!(facts.len(), 3);
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.relationship_type == "has_event")
            .count(),
        1
    );
}

#[tokio::test]
async fn on_cycle_succeeded_adopts_cursor_for_next_incremental_sync() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let delta_link = format!("{base}/me/events/delta?$deltatoken=token-2");
    // First cycle: full sync.
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$deltatoken", "token-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(&[], None)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-1", "Trip to Rome")],
            Some(&delta_link),
        )))
        .up_to_n_times(2)
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    store
        .store("calendar", &oauth_bundle("t0ken", None))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, "https://oauth.example.com/token"),
        Some(store),
        None,
        None,
        Some(http_client()),
    )
    .unwrap();

    let first = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(first.new_cursor.as_deref(), Some(delta_link.as_str()));
    // The in-memory marker must NOT advance until the supervisor confirms
    // the cycle (issue #314): a second sync before `on_cycle_succeeded`
    // re-requests the full delta.
    let pending = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(
        pending.fetched, 1,
        "marker must not advance before the cycle is confirmed"
    );
    connector
        .on_cycle_succeeded(first.new_cursor.as_deref())
        .await;
    // Now the incremental sync must request the stored delta link.
    let second = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(second.fetched, 0);
}

#[tokio::test]
async fn missing_delta_link_clears_marker_for_next_full_sync() {
    // A delta response without a final deltaLink tells the client to start
    // from scratch; the supervisor treats `new_cursor: None` as "unchanged",
    // so the connector must clear its in-memory marker itself to make the
    // next in-process cycle a full re-sync (self-healing, never skipping).
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let delta_link = format!("{base}/me/events/delta?$deltatoken=token-2");
    // Incremental requests with the adopted marker answer an empty delta
    // with NO deltaLink.
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(query_param("$deltatoken", "token-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(&[], None)))
        .mount(&server)
        .await;
    // Full-sync requests (no delta token) answer the initial delta link.
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(
            &[event_json("evt-1", "Trip to Rome")],
            Some(&delta_link),
        )))
        .up_to_n_times(2)
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    store
        .store("calendar", &oauth_bundle("t0ken", None))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, "https://oauth.example.com/token"),
        Some(store),
        None,
        None,
        Some(http_client()),
    )
    .unwrap();

    // Cycle 1: full sync, adopt the cursor.
    let first = connector.sync(SyncOptions::default()).await.unwrap();
    connector
        .on_cycle_succeeded(first.new_cursor.as_deref())
        .await;
    // Cycle 2: incremental, server returns no deltaLink → marker cleared.
    let second = connector.sync(SyncOptions::default()).await.unwrap();
    assert!(second.new_cursor.is_none());
    // Cycle 3: the marker must be gone, so this is a full sync again (the
    // no-token mock answers it with the event set).
    let third = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(third.fetched, 1);
    assert_eq!(third.new_cursor.as_deref(), Some(delta_link.as_str()));
}

#[tokio::test]
async fn config_rejects_app_password() {
    let err = GraphCalendarConnector::from_config(
        serde_json::json!({
            "auth": {"kind": "app_password", "username": "me@outlook.com"},
        }),
        None,
        None,
    )
    .err()
    .expect("app-password config must be rejected");
    assert!(
        err.to_string().contains("requires OAuth"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn resolve_auth_refreshes_expired_token_and_persists_bundle() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    let token_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "fresh-token",
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&token_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events/delta"))
        .and(header("authorization", "Bearer fresh-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delta_body(&[], None)))
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    let expired = Utc::now() - chrono::Duration::seconds(60);
    store
        .store("calendar", &oauth_bundle("stale-token", Some(expired)))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, &format!("{}/token", token_server.uri())),
        Some(store.clone()),
        None,
        None,
        Some(http_client()),
    )
    .unwrap();

    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 0);
    // The refreshed bundle must be persisted back to the store.
    let stored = store.load("calendar").await.unwrap().unwrap();
    let SecretBundle::OAuth { access_token, .. } = stored else {
        panic!("expected OAuth bundle");
    };
    assert_eq!(access_token, "fresh-token");
}

#[tokio::test]
async fn authenticate_401_reports_expired() {
    let server = MockServer::start().await;
    let base = format!("{}/v1.0", server.uri());
    Mock::given(method("GET"))
        .and(path("/v1.0/me/events"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    store
        .store("calendar", &oauth_bundle("t0ken", None))
        .await
        .unwrap();
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(&base, "https://oauth.example.com/token"),
        Some(store),
        None,
        None,
        Some(http_client()),
    )
    .unwrap();

    assert_eq!(
        connector.authenticate().await.unwrap(),
        ConnectorAuthState::Expired
    );
}

#[tokio::test]
async fn event_to_facts_maps_recurrence_and_timezone() {
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(GRAPH_BASE_URL, "https://oauth.example.com/token"),
        None,
        Some("Devansh".to_string()),
        None,
        None,
    )
    .unwrap();

    // singleInstance → no recurrence; unknown zone falls back to UTC.
    let mut single = event_json("evt-single", "Lunch");
    single["recurrence"] = serde_json::json!({"pattern": {"type": "singleInstance"}});
    single["start"] =
        serde_json::json!({"dateTime": "2025-07-03T09:00:00", "timeZone": "Europe/London"});
    let facts = connector.event_to_facts(&serde_json::from_value(single).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.recurrence, RecurrenceType::None);
    // 2025-07-03 is BST: 09:00 London = 08:00 UTC.
    assert_eq!(
        primary.valid_from,
        Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 0, 0).unwrap())
    );

    // yearly → RecurrenceType::Yearly.
    let mut yearly = event_json("evt-yearly", "Birthday");
    yearly["recurrence"] = serde_json::json!({"pattern": {"type": "absoluteYearly"}});
    let facts = connector.event_to_facts(&serde_json::from_value(yearly).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.recurrence, RecurrenceType::Yearly);

    // Fortnightly (weekly + interval 2) → the RRULE keeps INTERVAL so the
    // events subsystem advances every two weeks, not weekly.
    let mut fortnightly = event_json("evt-fortnightly", "Standup");
    fortnightly["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 2, "daysOfWeek": ["monday", "wednesday"]},
        "range": {"type": "noEnd"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(fortnightly).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.recurrence, RecurrenceType::Weekly);
    assert_eq!(primary.recurrence_interval, 2);
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE")
    );
    assert_eq!(primary.recurrence_until, None);

    // Relative-monthly (second Tuesday) → BYSETPOS + BYDAY are retained.
    let mut relative = event_json("evt-relative", "Board meeting");
    relative["recurrence"] = serde_json::json!({
        "pattern": {"type": "relativeMonthly", "interval": 1, "index": "second", "daysOfWeek": ["tuesday"]},
        "range": {"type": "noEnd"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(relative).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(primary.recurrence, RecurrenceType::Monthly);
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=MONTHLY;BYSETPOS=2;BYDAY=TU")
    );

    // Bounded series: a numbered range maps to COUNT, an endDate range to
    // UNTIL, so the series stops advancing instead of staying active forever.
    let mut numbered = event_json("evt-numbered", "Course");
    numbered["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 1},
        "range": {"type": "numbered", "numberOfOccurrences": 10}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(numbered).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;COUNT=10")
    );
    // 10 weekly occurrences from 2025-05-03 → the 10th is 2025-07-05.
    assert_eq!(
        primary.recurrence_until,
        Some(Utc.with_ymd_and_hms(2025, 7, 5, 9, 0, 0).unwrap())
    );

    let mut end_dated = event_json("evt-enddated", "Season");
    end_dated["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 1},
        "range": {"type": "endDate", "endDate": "2025-06-30"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(end_dated).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;UNTIL=20250630T235959Z")
    );
    assert_eq!(
        primary.recurrence_until,
        Some(Utc.with_ymd_and_hms(2025, 6, 30, 23, 59, 59).unwrap())
    );
}

#[tokio::test]
async fn event_to_facts_end_date_uses_recurrence_range_time_zone() {
    let connector = GraphCalendarConnector::from_config_with_http(
        oauth_config(GRAPH_BASE_URL, "https://oauth.example.com/token"),
        None,
        Some("Devansh".to_string()),
        None,
        None,
    )
    .unwrap();

    // The range's `recurrenceTimeZone` anchors the inclusive end-of-day
    // boundary: 23:59:59 London (BST, UTC+1) on 2025-06-30 is 22:59:59 UTC.
    let mut london = event_json("evt-london", "Season");
    london["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 1},
        "range": {"type": "endDate", "endDate": "2025-06-30", "recurrenceTimeZone": "Europe/London"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(london).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;UNTIL=20250630T225959Z")
    );
    assert_eq!(
        primary.recurrence_until,
        Some(Utc.with_ymd_and_hms(2025, 6, 30, 22, 59, 59).unwrap())
    );

    // A zone ahead of UTC must not leak the next local date into the
    // series: 23:59:59 Auckland (UTC+12) is 11:59:59 UTC, so the old
    // UTC-end-of-day bound would have included July 1st local hours.
    let mut auckland = event_json("evt-auckland", "Season");
    auckland["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 1},
        "range": {"type": "endDate", "endDate": "2025-06-30", "recurrenceTimeZone": "Pacific/Auckland"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(auckland).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;UNTIL=20250630T115959Z")
    );

    // Without `recurrenceTimeZone` the event time zone is the fallback: the
    // event starts in Europe/London, so the boundary is London end-of-day.
    let mut event_tz = event_json("evt-event-tz", "Season");
    event_tz["start"] =
        serde_json::json!({"dateTime": "2025-05-03T09:00:00", "timeZone": "Europe/London"});
    event_tz["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 1},
        "range": {"type": "endDate", "endDate": "2025-06-30"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(event_tz).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;UNTIL=20250630T225959Z")
    );

    // Without any time zone the boundary stays UTC end-of-day (unchanged
    // behaviour for ranges without a usable zone).
    let mut utc = event_json("evt-utc", "Season");
    utc["recurrence"] = serde_json::json!({
        "pattern": {"type": "weekly", "interval": 1},
        "range": {"type": "endDate", "endDate": "2025-06-30"}
    });
    let facts = connector.event_to_facts(&serde_json::from_value(utc).unwrap());
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .expect("primary has_event fact");
    assert_eq!(
        primary.recurrence_rule.as_deref(),
        Some("FREQ=WEEKLY;UNTIL=20250630T235959Z")
    );
}
