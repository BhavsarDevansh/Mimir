//! CalDAV calendar connector (Phase 3 C3 / #197), gated by the `calendar`
//! feature.
//!
//! A [`caldav::CalDavClient`] (PROPFIND + sync-collection REPORT, sync-token
//! incremental sync, `icalendar` VEVENT parsing) backs a [`CalendarConnector`]
//! implementing the two-step ingestion model ([`crate::Connector`]) in
//! `Polling` mode. Auth is an app password (HTTP Basic) or an OAuth bearer
//! token refreshed by the connector; the interactive PKCE login that obtains
//! the first token is A4 / #206.
//!
//! # C3 / C4 boundary
//!
//! C3 (#197) delivers the *transport* + `sync` that stages parsed VEVENTs in
//! an internal buffer. C4 / #198 (this module's extractor + write-back)
//! converts those events into a cluster of [`NormalizedFact`]s — a primary
//! `user has_event <event>` (typed [`EventType::Appointment`], recurrence
//! from `RRULE` `FREQ`), `<event> located_in <place>`, and `<attendee>
//! attending <event>` — so the shared `normalize_and_insert` pipeline
//! resolves every entity via F5 and the events-subsystem (#74) surfaces
//! future-dated / recurring events in the user's "Upcoming" section. C4
//! also adds the only connector write-back: `act()` creates/updates/deletes
//! remote events via CalDAV `PUT`/`DELETE`. Server-side deletions
//! (tombstones) are logged but not yet propagated to the KB (tracked as a
//! follow-up); the daemon `AppState` wiring + `mimir connector …` CLI land
//! in A1–A3 (#202–#204).
//!
//! # Credentials
//!
//! Per the [`crate::secrets`] design, the non-secret auth *method* + username
//! / OAuth client config live in `config_json`; the secret itself (app
//! password or OAuth token bundle) lives in the shared
//! [`SecretStore`](crate::secrets::SecretStore) under the connector slug. The
//! connector loads it by slug (the `__slug` the supervisor injects) and, for
//! OAuth, refreshes an expired access token against the configured token
//! endpoint, persisting the refreshed bundle back to the store.

pub mod caldav;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::calendar::caldav::RawCalDavEvent;
use crate::connector::{Connector, ConnectorContext, ConnectorError, ConnectorFactory};
use crate::secrets::SecretStore;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default poll interval for a CalDAV connector (15 min). CalDAV sync-token
/// servers are cheap to poll; 15 min balances freshness against rate limits.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Default jitter on the poll interval (±1 min) to avoid thundering-herd
/// syncs when several calendar instances run.
const DEFAULT_POLL_JITTER: Duration = Duration::from_secs(60);

const DEFAULT_SLUG: &str = "calendar";
const DEFAULT_DISPLAY_NAME: &str = "Calendar";

fn default_poll_interval_secs() -> u64 {
    DEFAULT_POLL_INTERVAL.as_secs()
}
fn default_poll_jitter_secs() -> u64 {
    DEFAULT_POLL_JITTER.as_secs()
}

// ---------------------------------------------------------------------------
// Config DTO (serde boundary for `config_json`)
// ---------------------------------------------------------------------------

/// The non-secret auth method + parameters, stored in `config_json`.
///
/// Tagged by `kind` so the on-disk JSON is self-describing:
/// `{"kind":"app_password","username":"..."}` or
/// `{"kind":"oauth","token_endpoint":"...","client_id":"..."}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CalendarAuthMethod {
    /// App-specific password (iCloud, Fastmail, Nextcloud). The `password`
    /// itself lives in the [`SecretStore`] as a
    /// [`SecretBundle::AppPassword`](crate::secrets::SecretBundle::AppPassword); only the username is non-secret.
    AppPassword {
        /// Account username / email.
        username: String,
    },
    /// OAuth 2.0 (Google Calendar). The access/refresh tokens live in the
    /// [`SecretStore`] as a
    /// [`SecretBundle::OAuth`](crate::secrets::SecretBundle::OAuth); only the client config is non-secret. The
    /// interactive PKCE login that obtains the first token is A4 / #206.
    #[serde(rename = "oauth")]
    OAuth {
        /// Token endpoint URL for refreshing the access token.
        token_endpoint: String,
        /// OAuth client id (public clients have no secret).
        client_id: String,
        /// OAuth client secret (optional for PKCE public clients).
        #[serde(default)]
        client_secret: Option<String>,
        /// Scope(s) to request on refresh, space-joined. Optional.
        #[serde(default)]
        scopes: Option<Vec<String>>,
    },
}

impl CalendarAuthMethod {
    /// The non-secret discriminant name (the serde `kind` tag), for error
    /// messages that must not `Debug`-format the OAuth `client_secret`.
    fn discriminant(&self) -> &'static str {
        match self {
            Self::AppPassword { .. } => "app_password",
            Self::OAuth { .. } => "oauth",
        }
    }
}

/// Deserialisable configuration for [`CalendarConnector`], stored as the
/// `config_json` of a `connectors` row (with `__slug` / `__ctype` /
/// `__instance_id` / `__cursor` injected by the supervisor). Unknown fields —
/// including the injected identity/cursor keys — are ignored by serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarConfigDto {
    /// Absolute URL of the CalDAV calendar collection to sync. Required.
    pub calendar_url: String,
    /// Non-secret auth method + parameters. Required.
    pub auth: CalendarAuthMethod,
    /// Poll interval in seconds. Defaults to 15 min.
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// Poll jitter in seconds. Defaults to 60 s.
    #[serde(default = "default_poll_jitter_secs")]
    pub poll_jitter_secs: u64,
    /// Display name override. Defaults to "Calendar".
    #[serde(default)]
    pub display_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// A CalDAV calendar connector (Phase 3 C3 / #197).
///
/// `Polling`-mode connector that syncs a CalDAV calendar collection via the
/// sync-token protocol and stages parsed VEVENTs in an internal buffer.
pub struct CalendarConnector {
    slug: String,
    display_name: String,
    config: CalendarConfigDto,
    /// Canonical user identity name (the `[identity] name`), injected via
    /// [`ConnectorContext::user_identity`]. When present, the extractor
    /// authors `user has_event <event>` (and the event surfaces in the
    /// user's "Upcoming" memory section); when `None`, the primary
    /// `has_event` fact is skipped and only the location/attendee facts
    /// are emitted (so the event does not surface in Upcoming).
    user_identity: Option<String>,
    /// Shared credential store (loaded by slug); `None` means the daemon did
    /// not wire one in (sync/authenticate then fail `NotAuthenticated`).
    secret_store: Option<Arc<dyn SecretStore>>,
    /// Shared HTTP client (auth applied per-request from the loaded bundle).
    http: reqwest::Client,
    /// In-memory incremental cursor (the last persisted sync-token). Seeded
    /// from `__cursor` at construction; the supervisor persists the value
    /// returned by [`sync`](Connector::sync) via `update_sync_cursor`.
    sync_token: Mutex<Option<String>>,
    /// Staged parsed VEVENTs awaiting extraction (drained by `extract`).
    buffer: Mutex<Vec<RawCalDavEvent>>,
}

/// Constructs [`CalendarConnector`] instances from their persisted
/// `config_json` + the shared [`SecretStore`] (Phase 3 C3 / #197).
pub struct CalendarConnectorFactory;

impl CalendarConnectorFactory {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CalendarConnectorFactory {
    fn default() -> Self {
        Self
    }
}

impl ConnectorFactory for CalendarConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError> {
        let cursor = config
            .get("__cursor")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let connector = CalendarConnector::from_config_with_http(
            config,
            ctx.secret_store.clone(),
            ctx.user_identity.clone(),
            cursor,
            None,
        )?;
        Ok(Arc::new(connector) as Arc<dyn Connector>)
    }
}

mod construct;
mod credentials;
mod payload;
mod sync;
mod trait_impl;
