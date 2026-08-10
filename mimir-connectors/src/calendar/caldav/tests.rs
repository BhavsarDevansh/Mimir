//! Unit tests for the CalDAV transport, XML parsing, and iCalendar decoding.

use super::xml::first_child_text;
use super::*;
use crate::connector::ConnectorError;
use chrono::{TimeZone as _, Utc};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http_client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

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
UID:birthday-mom@test\n\
SUMMARY:Mom's birthday\n\
DTSTART:20250101T090000Z\n\
RRULE:FREQ=YEARLY\n\
END:VEVENT\n\
END:VCALENDAR";

/// Build a multistatus body for a sync-collection response.
fn sync_body(token: &str, items: &[(&str, &str, Option<&str>)], deleted: &[&str]) -> String {
    let mut responses = String::new();
    for (href, ical, etag) in items {
        let etag_el = etag
            .map(|e| format!("<d:getetag>{e}</d:getetag>"))
            .unwrap_or_default();
        // calendar-data wrapped in CDATA so newlines survive verbatim.
        responses.push_str(&format!(
            "<d:response><d:href>{href}</d:href><d:propstat><d:prop>{etag_el}\
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

#[tokio::test]
async fn sync_collection_full_returns_events_and_token() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    Mock::given(method("REPORT"))
        .and(path("/cal/personal/"))
        .and(header("content-type", "application/xml; charset=utf-8"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body(
                    "token-2",
                    &[("/cal/uid-1.ics", ICAL_EVENT, Some("\"etag-1\""))],
                    &[],
                )),
        )
        .mount(&server)
        .await;

    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    );
    let res = client.sync_collection(&url, None).await.unwrap();
    assert_eq!(res.new_sync_token.as_deref(), Some("token-2"));
    assert_eq!(res.changed.len(), 1);
    let r = &res.changed[0];
    assert_eq!(r.href, "/cal/uid-1.ics");
    assert_eq!(r.etag.as_deref(), Some("\"etag-1\""));
    assert!(r.calendar_data.as_deref().unwrap().contains("Trip to Rome"));
    assert!(res.deleted.is_empty());
}

#[tokio::test]
async fn sync_collection_incremental_returns_only_changed_and_deletes() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());

    // First (full) sync.
    Mock::given(method("REPORT"))
        .and(path("/cal/personal/"))
        .and(body_string_contains("<d:sync-token/>"))
        .respond_with(ResponseTemplate::new(207).set_body_string(sync_body(
            "token-1",
            &[("/cal/a.ics", ICAL_EVENT, Some("\"ea\""))],
            &[],
        )))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Second (incremental) sync with stored token-1.
    Mock::given(method("REPORT"))
        .and(path("/cal/personal/"))
        .and(body_string_contains("<d:sync-token>token-1</d:sync-token>"))
        .respond_with(ResponseTemplate::new(207).set_body_string(sync_body(
            "token-2",
            &[("/cal/b.ics", ICAL_RECURRING, Some("\"eb\""))],
            &["/cal/a.ics"],
        )))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    );
    let full = client.sync_collection(&url, None).await.unwrap();
    assert_eq!(full.new_sync_token.as_deref(), Some("token-1"));
    assert_eq!(full.changed.len(), 1);

    let incr = client.sync_collection(&url, Some("token-1")).await.unwrap();
    assert_eq!(incr.new_sync_token.as_deref(), Some("token-2"));
    assert_eq!(incr.changed.len(), 1);
    assert_eq!(incr.changed[0].href, "/cal/b.ics");
    assert_eq!(incr.deleted, vec!["/cal/a.ics".to_string()]);
}

#[tokio::test]
async fn sync_collection_unauthenticated_returns_not_authenticated() {
    let server = MockServer::start().await;
    Mock::given(method("REPORT"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Bearer {
            token: "stale".into(),
        },
    );
    let err = client
        .sync_collection(&format!("{}/cal/", server.uri()), None)
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::NotAuthenticated), "{err:?}");
}

/// Regression guard: the `sync-collection` REPORT root element must be in
/// the `DAV:` namespace (`<d:sync-collection>`), per RFC 6578 §3.1 — not the
/// CalDAV namespace. Namespace-strict servers (sabre/dav: Nextcloud, ownCloud,
/// Baïkal; Google/Apple/Fastmail) reject `{caldav}sync-collection`. The mock
/// only matches a body containing `<d:sync-collection`, so a regression to
/// `<cal:sync-collection>` would not match and the request would error.
#[tokio::test]
async fn sync_collection_request_root_is_in_dav_namespace() {
    let server = MockServer::start().await;
    Mock::given(method("REPORT"))
        .and(body_string_contains("<d:sync-collection"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body(
                    "tok",
                    &[("/cal/a.ics", ICAL_EVENT, Some("\"e1\""))],
                    &[],
                )),
        )
        .mount(&server)
        .await;
    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    );
    let res = client
        .sync_collection(&format!("{}/cal/personal/", server.uri()), None)
        .await
        .expect("REPORT must use the DAV: namespace for the sync-collection root");
    assert_eq!(res.changed.len(), 1);
}

#[tokio::test]
async fn is_calendar_detects_calendar_collection() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\n\
<d:response><d:href>/cal/personal/</d:href><d:propstat><d:prop>\
<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
    Mock::given(method("PROPFIND"))
        .and(header("depth", "0"))
        .respond_with(ResponseTemplate::new(207).set_body_string(body))
        .mount(&server)
        .await;
    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    );
    assert!(client.is_calendar(&url).await.unwrap());
}

#[tokio::test]
async fn is_calendar_rejects_non_calendar_collection() {
    let server = MockServer::start().await;
    let url = format!("{}/notcal/", server.uri());
    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:multistatus xmlns:d=\"DAV:\">\n\
<d:response><d:href>/notcal/</d:href><d:propstat><d:prop>\
<d:resourcetype><d:collection/></d:resourcetype></d:prop>\
<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(207).set_body_string(body))
        .mount(&server)
        .await;
    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    );
    assert!(!client.is_calendar(&url).await.unwrap());
}

#[test]
fn parse_icalendar_extracts_fields_and_recur() {
    let events = parse_icalendar(ICAL_EVENT, "/cal/uid-1.ics", Some("\"e1\""));
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.vevent.uid.as_deref(), Some("uid-1@test"));
    assert_eq!(e.vevent.summary.as_deref(), Some("Trip to Rome"));
    assert_eq!(
        e.vevent.starts_at,
        Some(Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap())
    );
    assert_eq!(
        e.vevent.ends_at,
        Some(Utc.with_ymd_and_hms(2025, 5, 7, 18, 0, 0).unwrap())
    );
    assert!(e.vevent.attendees.is_empty());
    assert!(e.vevent.organizer.is_none());
    assert_eq!(e.vevent.location.as_deref(), Some("Rome"));
    assert_eq!(e.vevent.status.as_deref(), Some("CONFIRMED"));
    assert!(e.vevent.recurrence_rule.is_none());

    let rec = parse_icalendar(ICAL_RECURRING, "/cal/b.ics", None);
    assert_eq!(rec.len(), 1);
    assert_eq!(
        rec[0].vevent.recurrence_rule.as_deref(),
        Some("FREQ=YEARLY")
    );
}

#[test]
fn parse_icalendar_invalid_payload_returns_empty() {
    assert!(parse_icalendar("not ical at all", "/x.ics", None).is_empty());
    assert!(parse_icalendar("", "/x.ics", None).is_empty());
}

#[test]
fn parse_icalendar_extracts_attendees_organizer_and_tzid() {
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
    let events = parse_icalendar(ICAL, "/cal/meet-1.ics", None);
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(
        e.vevent.starts_at,
        Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 0, 0).unwrap())
    );
    assert_eq!(
        e.vevent.ends_at,
        Some(Utc.with_ymd_and_hms(2025, 7, 3, 8, 30, 0).unwrap())
    );
    assert_eq!(e.vevent.organizer.as_deref(), Some("Devansh Bhavsar"));
    // CN present → name; no CN → mailto value; empty CN → mailto value.
    assert_eq!(
        e.vevent.attendees,
        vec!["Alice", "bob@example.com", "empty@example.com"]
    );
}

// -----------------------------------------------------------------------
// Review-driven guards (PR #242): sync-level, tombstone gating, truncated
// responses, and split-text-node concatenation.
// -----------------------------------------------------------------------

#[tokio::test]
async fn sync_collection_request_includes_sync_level_element() {
    let server = MockServer::start().await;
    let url = format!("{}/cal/personal/", server.uri());
    Mock::given(method("REPORT"))
        .and(body_string_contains("<d:sync-level>1</d:sync-level>"))
        .respond_with(
            ResponseTemplate::new(207)
                .insert_header("content-type", "application/xml; charset=utf-8")
                .set_body_string(sync_body("tok", &[], &[])),
        )
        .mount(&server)
        .await;
    let client = CalDavClient::new(
        http_client(),
        CalDavAuth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    );
    let res = client.sync_collection(&url, None).await.expect("sync ok");
    assert!(!res.truncated);
}

#[test]
fn parse_sync_collection_tombstones_only_on_explicit_404_or_410() {
    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>
<d:multistatus xmlns:d=\"DAV:\">
<d:sync-token>t</d:sync-token>
<d:response><d:href>/cal/perm-denied.ics</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 403 Forbidden</d:status></d:propstat></d:response>
<d:response><d:href>/cal/locked.ics</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 423 Locked</d:status></d:propstat></d:response>
<d:response><d:href>/cal/gone.ics</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 410 Gone</d:status></d:propstat></d:response>
<d:response><d:href>/cal/notfound.ics</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
</d:multistatus>"
            .to_string();
    let res = parse_sync_collection(&body).expect("parse ok");
    // Only 404/410 become tombstones; 403/423 are skipped (not purged).
    let mut deleted = res.deleted.clone();
    deleted.sort();
    assert_eq!(
        deleted,
        vec!["/cal/gone.ics".to_string(), "/cal/notfound.ics".to_string()]
    );
    assert!(res.changed.is_empty());
    assert!(!res.truncated);
}

#[test]
fn parse_sync_collection_marks_truncated_on_507() {
    let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>
<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">
<d:sync-token>partial</d:sync-token>
<d:response><d:href>/cal/personal/</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:propstat></d:response>
<d:response><d:href>/cal/first.ics</d:href><d:propstat><d:prop><cal:calendar-data><![CDATA[{ICAL_EVENT}]]></cal:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"
        );
    let res = parse_sync_collection(&body).expect("parse ok");
    assert!(res.truncated, "507 must set truncated");
    assert_eq!(res.new_sync_token.as_deref(), Some("partial"));
    // The partial changed set is still returned for paging.
    assert_eq!(res.changed.len(), 1);
    assert!(res.deleted.is_empty());
}

#[test]
fn first_child_text_concatenates_split_text_nodes() {
    // Two text segments separated by a comment inside <calendar-data>; the
    // first-text-only behaviour of `Node::text()` would lose the second.
    let xml = "<?xml version=\"1.0\"?>
<d:response xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">
<d:calendar-data>part-1<!-- c --><cal:extra/>part-2</d:calendar-data>
</d:response>";
    let doc = roxmltree::Document::parse(xml).unwrap();
    let resp = doc
        .descendants()
        .find(|n| n.has_tag_name("response"))
        .unwrap();
    // Only the *direct* text children are joined; the nested <cal:extra/>
    // element's text is not pulled in, but both direct text segments are.
    let text = first_child_text(&resp, "calendar-data").expect("some text");
    assert_eq!(text, "part-1part-2");
}
