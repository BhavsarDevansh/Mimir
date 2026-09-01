//! Microsoft Graph calendar backend (issue #474), gated by the `calendar`
//! feature.
//!
//! A [`GraphClient`] (Microsoft Graph `GET /me/events/delta` with
//! `$deltatoken` incremental sync, JSON event parsing) backs a
//! [`GraphCalendarConnector`] implementing the two-step ingestion model
//! ([`crate::Connector`]) in `Polling` mode. Auth is OAuth 2.0 only — the
//! access token is refreshed by the connector through the shared
//! [`crate::oauth`] machinery; the interactive PKCE login that obtains the
//! first token is A4 / #205.
//!
//! # Fact shape
//!
//! The extractor converts each Graph event into the same cluster of
//! [`mimir_knowledge::normalize::NormalizedFact`]s the CalDAV backend authors — a primary
//! `user has_event <event>` (typed
//! [`mimir_knowledge::models::enums::EventType::Appointment`], recurrence from the Graph
//! recurrence pattern), `<event> located_in <place>`, and `<attendee>
//! attending <event>` — by mapping the event onto the shared
//! [`crate::ical::RawVEvent`] and delegating to
//! [`crate::ical::vevent_to_facts`] (DRY: the CalDAV and iMIP backends
//! author the same shapes). No LLM extraction. Server-side deletions
//! (`@removed` events in the delta response) are staged as tombstones and
//! propagated to the KB fact lifecycle via `extract_deletions` (issue #247).
//!
//! # Credentials
//!
//! Per the [`crate::secrets`] design, the non-secret OAuth client config
//! lives in `config_json`; the secret (the OAuth token bundle) lives in the
//! shared [`SecretStore`] under the connector
//! slug. The connector loads it by slug (the `__slug` the supervisor
//! injects) and refreshes an expired access token against the configured
//! token endpoint, persisting the refreshed bundle back to the store.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::calendar::CalendarAuthMethod;
use crate::calendar::graph::client::GraphEvent;
use crate::connector::{Connector, ConnectorContext, ConnectorError, ConnectorFactory};
use crate::oauth::OAuthHttpClient;
use crate::secrets::SecretStore;

mod client;
mod construct;
mod credentials;
mod sync;
mod trait_impl;

pub use client::GraphClient;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default poll interval for a Microsoft Graph calendar connector (15 min).
/// Graph delta syncs are cheap; 15 min balances freshness against rate
/// limits (the same default as the CalDAV backend).
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

/// Deserialisable configuration for [`GraphCalendarConnector`], stored as
/// the `config_json` of a `connectors` row (with `__slug` / `__ctype` /
/// `__instance_id` / `__cursor` injected by the supervisor). Unknown fields —
/// including the injected identity/cursor keys — are ignored by serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphCalendarConfigDto {
    /// Non-secret auth method + parameters. Microsoft Graph is OAuth-only:
    /// an `app_password` config is rejected at construction with a clear
    /// error (Microsoft retired app passwords for Graph).
    pub auth: CalendarAuthMethod,
    /// Microsoft Graph service root. Defaults to
    /// `https://graph.microsoft.com/v1.0`; override for national clouds
    /// (e.g. `https://graph.microsoft.us/v1.0`) or test servers.
    #[serde(default)]
    pub base_url: Option<String>,
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

/// A Microsoft Graph calendar connector (issue #474).
///
/// `Polling`-mode connector that syncs the user's default calendar via the
/// Graph events delta query and stages parsed events in an internal buffer.
pub struct GraphCalendarConnector {
    slug: String,
    display_name: String,
    config: GraphCalendarConfigDto,
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
    /// OAuth HTTP client for token refresh (issue #240): the `oauth2`-crate
    /// adapter over the workspace reqwest 0.13 client, built with redirects
    /// disabled so a credential POST can never be bounced to another host.
    oauth_http: Option<OAuthHttpClient>,
    /// In-memory incremental cursor (the last confirmed `@odata.deltaLink`).
    /// Seeded from `__cursor` at construction; the supervisor persists the
    /// value returned by [`sync`](Connector::sync) via
    /// `update_sync_progress_and_durable_state` and hands it back through
    /// [`Connector::on_cycle_succeeded`] only after a fully successful
    /// cycle, so a cycle that fails after `sync` re-syncs from the last
    /// confirmed cursor on the next in-process cycle (issue #314).
    delta_link: Mutex<Option<String>>,
    /// Staged Graph events awaiting extraction (drained by `extract`).
    buffer: Mutex<Vec<GraphEvent>>,
    /// Staged server-side deletions — the event ids of `@removed` events the
    /// server reported deleted since the prior delta (drained by
    /// `extract_deletions`, issue #247).
    tombstones: Mutex<Vec<String>>,
}

/// Constructs [`GraphCalendarConnector`] instances from their persisted
/// `config_json` + the shared [`SecretStore`] (issue #474).
pub struct GraphCalendarConnectorFactory;

impl GraphCalendarConnectorFactory {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GraphCalendarConnectorFactory {
    fn default() -> Self {
        Self
    }
}

impl ConnectorFactory for GraphCalendarConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError> {
        let cursor = config
            .get("__cursor")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let connector = GraphCalendarConnector::from_config_with_http(
            config,
            ctx.secret_store.clone(),
            ctx.user_identity.clone(),
            cursor,
            None,
        )?;
        Ok(Arc::new(connector) as Arc<dyn Connector>)
    }
}
