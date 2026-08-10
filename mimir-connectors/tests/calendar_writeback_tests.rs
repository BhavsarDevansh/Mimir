//! CalDAV connector write-back (act) integration tests.
//!
//! Gated behind the `calendar` feature; `cargo test --no-default-features`
//! skips this file entirely.

#![cfg(feature = "calendar")]

use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate};

use mimir_connectors::{Connector, ConnectorAction, ConnectorError};

mod common;
use common::*;

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
