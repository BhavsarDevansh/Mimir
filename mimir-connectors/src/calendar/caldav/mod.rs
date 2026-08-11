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
//! is A4 / #205; this transport only consumes a token and the connector
//! refreshes it.
//!
//! # No `unsafe`
//!
//! This module honours the workspace `#![deny(unsafe_code)]` guarantee. All
//! HTTP, XML, and iCalendar parsing is delegated to safe, audited crates.

mod client;
mod ical;
#[cfg(test)]
mod tests;
mod xml;

pub use ical::{RawCalDavEvent, parse_icalendar};
// Re-exported for the sibling client/test modules; not part of the public surface.
use xml::{
    parse_resourcetype_is_calendar, parse_sync_collection, propfind_method, report_method,
    xml_escape,
};

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Resource + sync result
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------
pub struct CalDavClient {
    http: reqwest::Client,
    auth: CalDavAuth,
}

// ---------------------------------------------------------------------------
// PUT result
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutEventResult {
    /// The resource href written (the request URL).
    pub href: String,
    /// New `ETag` returned by the server, if any.
    pub etag: Option<String>,
}
