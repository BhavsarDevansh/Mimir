//! WebDAV XML parsing helpers for CalDAV responses.
//!
//! Parses `resourcetype` PROPFIND responses and RFC 6578 sync-collection
//! multistatus bodies with `roxmltree`, matching element *local* names so the
//! varied namespace prefixes servers use (`D:`/`d:`/`cal:`/`C:`) are tolerated.

use crate::calendar::caldav::{CalDavResource, SyncCollectionResult};
use crate::connector::ConnectorError;
use tracing::warn;

pub(super) fn propfind_method() -> reqwest::Method {
    reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method token")
}

/// REPORT HTTP method (not in reqwest's predefined set).
pub(super) fn report_method() -> reqwest::Method {
    reqwest::Method::from_bytes(b"REPORT").expect("REPORT is a valid HTTP method token")
}

// ---------------------------------------------------------------------------
// XML helpers (roxmltree DOM, local-name matching)
// ---------------------------------------------------------------------------

/// Whether a `resourcetype` payload declares a CalDAV calendar collection.
pub(super) fn parse_resourcetype_is_calendar(xml: &str) -> bool {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return false;
    };
    doc.descendants()
        .any(|n| n.has_tag_name("resourcetype") && n.children().any(|c| c.has_tag_name("calendar")))
}

/// Parse a multistatus sync-collection response.
pub(super) fn parse_sync_collection(xml: &str) -> Result<SyncCollectionResult, ConnectorError> {
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
pub(super) fn first_child_text(node: &roxmltree::Node, tag: &str) -> Option<String> {
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
pub(super) fn response_status_code(node: &roxmltree::Node) -> Option<u16> {
    first_child_text(node, "status")
        .and_then(|s| s.split_whitespace().nth(1).map(str::to_string))
        .and_then(|code| code.parse().ok())
}

/// A href is a "changed resource" candidate iff it does not end with `/`
/// (i.e. it is not a collection). This guards against the calendar collection
/// itself appearing in a Depth-1 response.
pub(super) fn href_is_resource(href: &str) -> bool {
    !href.ends_with('/')
}

/// Minimal XML text escaping for embedding a sync-token in a request body.
pub(super) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// iCalendar parsing -> RawCalDavEvent
// ---------------------------------------------------------------------------
