//! CalDAV transport client (Phase 3 C3 / #197).
//!
//! A small, dependency-light CalDAV client built on the workspace's existing
//! `reqwest` 0.13 client. It speaks the two CalDAV verbs this connector needs:
//!
//! - **PROPFIND** (Depth 0, `resourcetype`) — calendar verification / health.
//! - **sync-collection REPORT** (RFC 6578) — fetches changed VEVENTs and a
//!   new `sync-token` in one round trip, requesting `<cal:calendar-data/>`
//!   inline so no follow-up multiget is needed. Omitting the `<sync-token>`
//!   element performs a full sync and yields the initial token; including it
//!   performs an incremental sync (no full re-fetch). The persisted sync-token
//!   is the connector's incremental cursor.
//!
//! WebDAV XML is parsed with `roxmltree`, a read-only DOM parser that matches
//! element *local* names, so the varied namespace prefixes servers use
//! (`D:`/`d:`/`cal:`/`C:`) are tolerated. iCalendar payloads are parsed with
//! `icalendar` into typed [`RawCalDavEvent`]s.
//!
//! # Auth
//!
//! Two credential kinds, mirroring [`crate::secrets::SecretBundle`]:
//! [`CalDavAuth::Basic`] (app password — iCloud, Fastmail, Nextcloud) and
//! [`CalDavAuth::Bearer`] (OAuth access token — Google, refreshed by the
//! connector). The interactive PKCE login that *obtains* the first OAuth token
//! is A4 / #206; this transport only consumes a token and the connector
//! refreshes it.
//!
//! # No `unsafe`
//!
//! This module honours the workspace `#![deny(unsafe_code)]` guarantee. All
//! HTTP, XML, and iCalendar parsing is delegated to safe, audited crates.

use std::time::Duration;

use tracing::warn;

use crate::connector::ConnectorError;

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Credentials presented to the CalDAV server for every request.
#[derive(Debug, Clone)]
pub enum CalDavAuth {
    /// HTTP Basic auth (app-specific password: iCloud, Fastmail, Nextcloud).
    Basic {
        /// Account username / email.
        username: String,
        /// App-specific password.
        password: String,
    },
    /// OAuth 2.0 bearer token (Google Calendar via refresh, #197).
    Bearer {
        /// Short-lived access token.
        token: String,
    },
}

// ---------------------------------------------------------------------------
// Resource + sync result
// ---------------------------------------------------------------------------

/// A single CalDAV resource (one VEVENT-bearing `.ics` href).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalDavResource {
    /// The resource href (server-relative or absolute), the CalDAV item id.
    pub href: String,
    /// `ETag` of the resource, if the server returned one.
    pub etag: Option<String>,
    /// The raw iCalendar payload (`VCALENDAR` text), if present (deleted
    /// resources carry none).
    pub calendar_data: Option<String>,
}

/// Outcome of a sync-collection REPORT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCollectionResult {
    /// The new sync-token to persist as the cursor for the next incremental
    /// sync. `None` if the server did not return one (treat the next cycle as
    /// a full re-sync).
    pub new_sync_token: Option<String>,
    /// Changed/new resources with their iCalendar payload.
    pub changed: Vec<CalDavResource>,
    /// Hrefs the server reports as deleted since the prior token.
    pub deleted: Vec<String>,
    /// Whether the server signalled a truncated (RFC 6578 §6.5) response —
    /// an HTTP 507 `Insufficient Storage` status on a `<response>`. When
    /// `true`, `new_sync_token` is the partial cursor and the caller must
    /// re-request with it to page through the remaining changes until the
    /// collection is drained (a `false` value completes the sync).
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A CalDAV client wrapping a configured `reqwest` client.
pub struct CalDavClient {
    http: reqwest::Client,
    auth: CalDavAuth,
}

impl CalDavClient {
    /// Build a client over the supplied HTTP client and credentials.
    pub fn new(http: reqwest::Client, auth: CalDavAuth) -> Self {
        Self { http, auth }
    }

    /// Build a client with a default HTTP backend (30 s timeout) and the given
    /// credentials.
    pub fn with_default_http(auth: CalDavAuth) -> Result<Self, ConnectorError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ConnectorError::Config(format!("HTTP client build failed: {e}")))?;
        Ok(Self::new(http, auth))
    }

    /// Apply the configured auth to a request builder.
    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            CalDavAuth::Basic { username, password } => {
                builder.basic_auth(username, Some(password))
            }
            CalDavAuth::Bearer { token } => builder.bearer_auth(token),
        }
    }

    /// PROPFIND (Depth 0) requesting `resourcetype`; returns whether the URL is
    /// a CalDAV calendar collection. Used for health probing.
    pub async fn is_calendar(&self, calendar_url: &str) -> Result<bool, ConnectorError> {
        let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:propfind xmlns:d=\"DAV:\">\n  <d:prop>\n    <d:resourcetype/>\n  </d:prop>\n</d:propfind>";
        let resp = self
            .authed(self.http.request(propfind_method(), calendar_url))
            .header("Depth", "0")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/xml; charset=utf-8",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        if !status.is_success() && status.as_u16() != 207 {
            return Err(ConnectorError::Other(format!(
                "PROPFIND failed: HTTP {status}"
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        Ok(parse_resourcetype_is_calendar(&text))
    }

    /// sync-collection REPORT. `sync_token = None` performs a full sync and
    /// yields the initial token; `Some(token)` performs an incremental sync.
    pub async fn sync_collection(
        &self,
        calendar_url: &str,
        sync_token: Option<&str>,
    ) -> Result<SyncCollectionResult, ConnectorError> {
        let token_element = match sync_token {
            Some(t) => format!("<d:sync-token>{}</d:sync-token>", xml_escape(t)),
            None => "<d:sync-token/>".to_string(),
        };
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:sync-collection xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\n  \
{token_element}\n  <d:sync-level>1</d:sync-level>\n  <d:prop>\n    <d:getetag/>\n    <cal:calendar-data/>\n  </d:prop>\n\
</d:sync-collection>"
        );
        let resp = self
            .authed(self.http.request(report_method(), calendar_url))
            .header("Depth", "1")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/xml; charset=utf-8",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        // RFC 6578 §6.5: a truncated result set is signalled with HTTP 507
        // (Insufficient Storage) carrying a partial multistatus body plus an
        // advancing `sync-token`; accept it alongside 207 and parse the body
        // so the caller can page with the new token.
        if !status.is_success() && status.as_u16() != 207 && status.as_u16() != 507 {
            return Err(ConnectorError::Other(format!(
                "sync-collection REPORT failed: HTTP {status}"
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        parse_sync_collection(&text)
    }
}

/// PROPFIND HTTP method (not in reqwest's predefined set). Constructed once at
/// call time: `http::Method::from_bytes` is not `const`, and the token is a
/// valid HTTP method so construction cannot fail.
fn propfind_method() -> reqwest::Method {
    reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method token")
}

/// REPORT HTTP method (not in reqwest's predefined set).
fn report_method() -> reqwest::Method {
    reqwest::Method::from_bytes(b"REPORT").expect("REPORT is a valid HTTP method token")
}

// ---------------------------------------------------------------------------
// XML helpers (roxmltree DOM, local-name matching)
// ---------------------------------------------------------------------------

/// Whether a `resourcetype` payload declares a CalDAV calendar collection.
fn parse_resourcetype_is_calendar(xml: &str) -> bool {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return false;
    };
    doc.descendants()
        .any(|n| n.has_tag_name("resourcetype") && n.children().any(|c| c.has_tag_name("calendar")))
}

/// Parse a multistatus sync-collection response.
fn parse_sync_collection(xml: &str) -> Result<SyncCollectionResult, ConnectorError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| ConnectorError::Parse(format!("CalDAV XML parse error: {e}")))?;
    let mut result = SyncCollectionResult {
        new_sync_token: None,
        changed: Vec::new(),
        deleted: Vec::new(),
        truncated: false,
    };
    // The sync-token is a direct child of multistatus (there is exactly one).
    if let Some(tok) = doc.descendants().find(|n| n.has_tag_name("sync-token")) {
        result.new_sync_token = tok.text().map(str::to_string);
    }
    for resp in doc.descendants().filter(|n| n.has_tag_name("response")) {
        let Some(href) = first_child_text(&resp, "href") else {
            continue;
        };
        // RFC 6578 §6.5: a 507 status on a `<response>` marks a truncated
        // result set — the server still returns a valid, advancing
        // `sync-token` and the partial changes so far. Record truncation and
        // keep paging; the 507 `<response>` itself carries no item to stage.
        let status_code = response_status_code(&resp);
        if status_code == Some(507) {
            result.truncated = true;
            continue;
        }
        // Collection hrefs (trailing `/`) and empty hrefs are skipped: only
        // item hrefs are staged or tombstoned.
        if !href_is_resource(&href) {
            continue;
        }
        // We requested `<cal:calendar-data/>` inline, so its presence marks a
        // live (changed/new) resource. Its absence is a deletion *only* when
        // the server explicitly reports 404/410 — a 403 (permission denied),
        // 423 (locked), or any other error has no `calendar-data` either, and
        // tombstoning on it would purge a live event once C4 wires deletions
        // to fact lifecycle. Unexpected statuses are logged and skipped.
        let caldata = first_child_text(&resp, "calendar-data");
        if let Some(caldata) = caldata {
            result.changed.push(CalDavResource {
                href,
                etag: first_child_text(&resp, "getetag"),
                calendar_data: Some(caldata),
            });
        } else {
            match status_code {
                Some(404) | Some(410) => result.deleted.push(href),
                Some(code) => warn!(
                    href = %href,
                    status = code,
                    "CalDAV response carried no calendar-data with an unexpected status; not tombstoning"
                ),
                None => warn!(
                    href = %href,
                    "CalDAV response carried no calendar-data and no status; not tombstoning"
                ),
            }
        }
    }
    Ok(result)
}

/// First descendant element of `node` matching a local tag name, returning its
/// trimmed text content (if any). Used for the leaf `<href>`/`<getetag>`/
/// `<calendar-data>`/`<status>` children of a `<response>`.
///
/// `roxmltree::Node::text()` returns only the *first* text child, so a
/// `calendar-data`/`summary`/`calendar-name` element whose text is split across
/// multiple text/CDATA segments would be silently truncated. All direct text
/// children are concatenated and trimmed instead, matching the documented
/// behaviour and avoiding lossy parsing of server responses.
fn first_child_text(node: &roxmltree::Node, tag: &str) -> Option<String> {
    node.descendants()
        .find(|n| n.has_tag_name(tag))
        .map(|n| {
            n.children()
                .filter(|c| c.is_text())
                .map(|c| c.text().unwrap_or_default())
                .collect::<String>()
        })
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Parse the HTTP status code from a `<response>`'s `<status>` child (e.g.
/// `HTTP/1.1 404 Not Found` → `404`). Returns `None` when there is no
/// `<status>` element or the second token is not a parseable code.
fn response_status_code(node: &roxmltree::Node) -> Option<u16> {
    first_child_text(node, "status")
        .and_then(|s| s.split_whitespace().nth(1).map(str::to_string))
        .and_then(|code| code.parse().ok())
}

/// A href is a "changed resource" candidate iff it does not end with `/`
/// (i.e. it is not a collection). This guards against the calendar collection
/// itself appearing in a Depth-1 response.
fn href_is_resource(href: &str) -> bool {
    !href.ends_with('/')
}

/// Minimal XML text escaping for embedding a sync-token in a request body.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// iCalendar parsing -> RawCalDavEvent
// ---------------------------------------------------------------------------

/// A parsed VEVENT staged in the connector's buffer (Phase 3 C3 / #197).
///
/// Field values are the raw iCalendar property strings (e.g. `DTSTART` value
/// `"20250503T090000Z"` or with a `TZID` parameter). C4 / #198 converts these
/// into `NormalizedFact`s with full temporal/recurrence resolution; #197 only
/// parses + stages them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCalDavEvent {
    /// The CalDAV resource href (the item id).
    pub href: String,
    /// ETag, if known.
    pub etag: Option<String>,
    /// `UID` property.
    pub uid: Option<String>,
    /// `SUMMARY` property.
    pub summary: Option<String>,
    /// `DTSTART` property value (raw).
    pub dtstart: Option<String>,
    /// `DTEND` property value (raw).
    pub dtend: Option<String>,
    /// `LOCATION` property.
    pub location: Option<String>,
    /// `DESCRIPTION` property.
    pub description: Option<String>,
    /// `STATUS` property.
    pub status: Option<String>,
    /// `RRULE` property (recurrence; C4 / events-subsystem #74 owns this).
    pub recurrence_rule: Option<String>,
    /// The raw iCalendar payload, retained for C4's deeper extraction.
    pub raw_ical: String,
}

/// Parse an iCalendar payload into the VEVENTs it contains.
///
/// Returns one [`RawCalDavEvent`] per `VEVENT`. An empty/invalid payload yields
/// an empty vec (the connector logs and skips rather than failing the sync),
/// so one malformed event never aborts a whole sync.
pub fn parse_icalendar(ical: &str, href: &str, etag: Option<&str>) -> Vec<RawCalDavEvent> {
    // The low-level parser (`icalendar::parser`) yields a zero-copy
    // `Calendar` whose top-level `components` are the VEVENT/VTODO/VTIMEZONE
    // entries. We walk the low-level representation directly (the high-level
    // `icalendar::Calendar` is builder-oriented and has no parse-from-str
    // path in 0.17.x). `find_prop` + `ParseString::as_str` give owned copies so
    // the staged events outlive the borrowed input.
    use icalendar::parser::read_calendar;
    let Ok(calendar) = read_calendar(ical) else {
        return Vec::new();
    };
    calendar
        .components
        .iter()
        .filter(|c| c.name.as_str() == "VEVENT")
        .map(|event| {
            let prop = |key: &str| event.find_prop(key).map(|p| p.val.as_str().to_string());
            RawCalDavEvent {
                href: href.to_string(),
                etag: etag.map(str::to_string),
                uid: prop("UID"),
                summary: prop("SUMMARY"),
                dtstart: prop("DTSTART"),
                dtend: prop("DTEND"),
                location: prop("LOCATION"),
                description: prop("DESCRIPTION"),
                status: prop("STATUS"),
                recurrence_rule: prop("RRULE"),
                raw_ical: ical.to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(e.uid.as_deref(), Some("uid-1@test"));
        assert_eq!(e.summary.as_deref(), Some("Trip to Rome"));
        assert_eq!(e.dtstart.as_deref(), Some("20250503T090000Z"));
        assert_eq!(e.dtend.as_deref(), Some("20250507T180000Z"));
        assert_eq!(e.location.as_deref(), Some("Rome"));
        assert_eq!(e.status.as_deref(), Some("CONFIRMED"));
        assert!(e.recurrence_rule.is_none());

        let rec = parse_icalendar(ICAL_RECURRING, "/cal/b.ics", None);
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].recurrence_rule.as_deref(), Some("FREQ=YEARLY"));
    }

    #[test]
    fn parse_icalendar_invalid_payload_returns_empty() {
        assert!(parse_icalendar("not ical at all", "/x.ics", None).is_empty());
        assert!(parse_icalendar("", "/x.ics", None).is_empty());
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
}
