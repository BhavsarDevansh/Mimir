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

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::calendar::caldav::{
    CalDavAuth, CalDavClient, RawCalDavEvent, SyncCollectionResult, parse_icalendar,
};
use crate::connector::{
    ActionResult, Connector, ConnectorAction, ConnectorContext, ConnectorError, ConnectorFactory,
    ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
use crate::secrets::{SecretBundle, SecretStore};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

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
    /// itself lives in the [`SecretStore`](crate::secrets::SecretStore) as a
    /// [`SecretBundle::AppPassword`]; only the username is non-secret.
    AppPassword {
        /// Account username / email.
        username: String,
    },
    /// OAuth 2.0 (Google Calendar). The access/refresh tokens live in the
    /// [`SecretStore`](crate::secrets::SecretStore) as a
    /// [`SecretBundle::OAuth`]; only the client config is non-secret. The
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

impl CalendarConnector {
    /// Build a connector from its parsed configuration, a shared secret store
    /// (optional), and the supervisor-injected cursor.
    pub fn from_config(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        cursor: Option<String>,
    ) -> Result<Self, ConnectorError> {
        Self::from_config_with_http(config, secret_store, None, cursor, None)
    }

    /// Build a connector, allowing an injected `http` client (tests inject a
    /// client pointed at a mock server; production passes `None` for a default
    /// 30 s-timeout client).
    pub fn from_config_with_http(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
        http: Option<reqwest::Client>,
    ) -> Result<Self, ConnectorError> {
        // Recover the supervisor-injected slug before parsing the DTO: serde
        // ignores unknown fields (the DTO has no `deny_unknown_fields`), so the
        // injected `__slug` / `__cursor` keys pass through harmlessly.
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());
        let dto: CalendarConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid calendar config: {e}")))?;
        let http = match http {
            Some(c) => c,
            None => reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| ConnectorError::Config(format!("HTTP client build failed: {e}")))?,
        };
        Ok(Self {
            slug,
            display_name: dto
                .display_name
                .clone()
                .unwrap_or_else(|| DEFAULT_DISPLAY_NAME.to_string()),
            config: dto,
            user_identity: user_identity
                .filter(|n| !n.trim().is_empty())
                .map(|n| n.trim().to_string()),
            secret_store,
            http,
            sync_token: Mutex::new(cursor.filter(|c| !c.is_empty())),
            buffer: Mutex::new(Vec::new()),
        })
    }

    /// Build a [`CalDavClient`] from the current credentials.
    ///
    /// Loads the [`SecretBundle`] by slug; for OAuth, refreshes an expired
    /// access token first. Returns the client and the (possibly refreshed)
    /// bundle so the caller can persist it.
    async fn client_from_credentials(
        &self,
    ) -> Result<(CalDavClient, Option<SecretBundle>), ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?
            .ok_or(ConnectorError::NotAuthenticated)?;
        let (auth, refreshed) = self.resolve_auth(&bundle).await?;
        Ok((CalDavClient::new(self.http.clone(), auth), refreshed))
    }

    /// Turn a [`SecretBundle`] into a [`CalDavAuth`], refreshing an expired
    /// OAuth token when needed. Returns the auth and the refreshed bundle (if
    /// a refresh happened) for the caller to persist.
    async fn resolve_auth(
        &self,
        bundle: &SecretBundle,
    ) -> Result<(CalDavAuth, Option<SecretBundle>), ConnectorError> {
        match (&self.config.auth, bundle) {
            (
                CalendarAuthMethod::AppPassword { username },
                SecretBundle::AppPassword { password },
            ) => Ok((
                CalDavAuth::Basic {
                    username: username.clone(),
                    password: password.clone(),
                },
                None,
            )),
            (
                CalendarAuthMethod::OAuth { .. },
                SecretBundle::OAuth {
                    access_token,
                    refresh_token,
                    expires_at,
                },
            ) => {
                // Refresh if expired (or within a 60 s skew of expiry). An
                // unknown expiry (`None`) does not force a refresh on every
                // cycle — that would triple the POSTs against the token
                // endpoint and invite rate limiting. The token is reused
                // as-is; if it is actually expired the server returns 401 and
                // the next cycle re-authenticates.
                let needs_refresh = expires_at
                    .map(|exp| exp <= Utc::now() + chrono::Duration::seconds(60))
                    .unwrap_or(false);
                if needs_refresh {
                    let refresh_token = refresh_token.clone().ok_or_else(|| {
                        ConnectorError::Authentication(
                            "OAuth access token expired and no refresh token is stored".into(),
                        )
                    })?;
                    let refreshed = self.refresh_oauth(&refresh_token).await?;
                    let token = refreshed.access_token.clone().ok_or_else(|| {
                        ConnectorError::Authentication(
                            "token endpoint returned no access_token".into(),
                        )
                    })?;
                    let auth = CalDavAuth::Bearer { token };
                    Ok((auth, Some(refreshed.into_bundle(Some(refresh_token)))))
                } else {
                    Ok((
                        CalDavAuth::Bearer {
                            token: access_token.clone(),
                        },
                        None,
                    ))
                }
            }
            // Auth method / bundle kind mismatch — e.g. an app-password bundle
            // configured as OAuth, or vice versa.
            _ => Err(ConnectorError::Authentication(format!(
                "auth method {} does not match stored secret kind",
                self.config.auth.discriminant()
            ))),
        }
    }

    /// Refresh an OAuth access token via the configured token endpoint.
    ///
    /// Delegates to the shared [`crate::oauth::refresh_token`] helper so the
    /// Calendar and Email connectors share one refresh implementation (DRY).
    /// The `oauth2` crate is avoided: it depends on reqwest 0.12, which would
    /// duplicate the workspace's reqwest 0.13 stack; a refresh is a single
    /// form-encoded HTTPS POST returning JSON. The interactive PKCE login
    /// that *obtains* the first token is A4 / #206.
    async fn refresh_oauth(
        &self,
        refresh_token: &str,
    ) -> Result<crate::oauth::RefreshTokenResponse, ConnectorError> {
        let CalendarAuthMethod::OAuth {
            token_endpoint,
            client_id,
            client_secret,
            scopes,
        } = &self.config.auth
        else {
            return Err(ConnectorError::Config(
                "refresh_oauth called for a non-OAuth connector".into(),
            ));
        };
        crate::oauth::refresh_token(
            &self.http,
            token_endpoint,
            client_id,
            client_secret.as_deref(),
            scopes.as_deref(),
            refresh_token,
        )
        .await
    }

    /// Persist a refreshed OAuth bundle back to the secret store.
    async fn persist_refreshed(&self, bundle: &SecretBundle) -> Result<(), ConnectorError> {
        if let Some(store) = &self.secret_store {
            store.store(&self.slug, bundle).await.map_err(|e| {
                ConnectorError::Authentication(format!("secret persist failed: {e}"))
            })?;
        }
        Ok(())
    }

    /// Stage the changed resources of a sync result into the buffer (parsed),
    /// returning the number of VEVENTs staged.
    async fn stage(&self, result: SyncCollectionResult) -> Result<u32, ConnectorError> {
        let mut count = 0u32;
        for res in result.changed {
            if let Some(ical) = &res.calendar_data {
                let events = parse_icalendar(ical, &res.href, res.etag.as_deref());
                if events.is_empty() {
                    warn!(href = %res.href, "CalDAV resource had no parseable VEVENT");
                }
                count = count.saturating_add(events.len() as u32);
                self.buffer.lock().await.extend(events);
            } else {
                debug!(href = %res.href, "CalDAV changed resource had no calendar-data; skipping");
            }
        }
        for href in &result.deleted {
            // Server-side deletions (tombstones) are logged but not yet
            // propagated to the KB: surfacing a deletion needs a way for the
            // connector to report removals (extract only yields facts), so
            // trashing the corresponding facts is tracked as a follow-up.
            debug!(href = %href, "CalDAV reports deleted event; fact lifecycle deferred");
        }
        Ok(count)
    }

    /// Convert one staged VEVENT into its cluster of [`NormalizedFact`]s.
    ///
    /// Emits up to three fact shapes, all resolved by `normalize_and_insert`:
    /// 1. `user has_event <event>` — the primary appointment, carrying the
    ///    temporal bounds, the recurrence (`RRULE` `FREQ`), and an
    ///    [`EventType::Appointment`] hint so the events-subsystem overlay is
    ///    typed correctly. Authored only when a user identity is injected
    ///    (so the event surfaces in the user's "Upcoming" section); without
    ///    one the primary fact is skipped and the event is captured via its
    ///    location/attendee facts instead.
    /// 2. `<event> located_in <place>` — the `LOCATION` resolves to a
    ///    `Place` entity via the full F5 chain (no `entity_locations` overlay;
    ///    a calendar venue is a property of the event, not the user's
    ///    location history, so it does not bloat `Visited` rows). Carries no
    ///    temporal bounds, so it spawns no events-subsystem overlay.
    /// 3. `<attendee> attending <event>` — each `ATTENDEE` resolves to a
    ///    `Person` entity via F5. Like the location fact it carries no
    ///    temporal bounds and spawns no overlay.
    fn event_to_facts(&self, event: &RawCalDavEvent) -> Vec<NormalizedFact> {
        // The VEVENT → fact cluster (`has_event` / `located_in` / `attending`)
        // is shared with the Email iMIP extraction in
        // [`crate::ical::vevent_to_facts`] (DRY). The CalDAV connector supplies
        // the user identity and the provenance `raw_reference` (the VEVENT UID,
        // falling back to the resource href) and delegates; entity resolution,
        // confidence, and the events-subsystem overlay run in the shared
        // `normalize_and_insert` pipeline.
        let raw_ref = event
            .vevent
            .uid
            .clone()
            .unwrap_or_else(|| event.href.clone());
        crate::ical::vevent_to_facts(self.user_identity.as_deref(), &event.vevent, &raw_ref)
    }

    /// Reject an event `href` that points outside the configured calendar
    /// collection, so a caller-supplied URL cannot redirect the stored
    /// credentials (Basic/Bearer auth, attached by `CalDavClient`) to another
    /// host or an unrelated resource. The check is origin-aware: the scheme,
    /// host, and port must match the configured `calendar_url`, and the path
    /// must lie under the calendar collection.
    fn ensure_in_calendar(&self, href: &str) -> Result<(), ConnectorError> {
        let base = reqwest::Url::parse(self.config.calendar_url.trim_end_matches('/'))
            .map_err(|e| ConnectorError::Config(format!("invalid calendar_url: {e}")))?;
        let target = reqwest::Url::parse(href)
            .map_err(|e| ConnectorError::Config(format!("invalid event href `{href}`: {e}")))?;
        let same_origin = base.scheme() == target.scheme()
            && base.host_str() == target.host_str()
            && base.port() == target.port();
        if !same_origin {
            return Err(ConnectorError::Config(format!(
                "href `{href}` is outside the configured calendar origin"
            )));
        }
        let base_path = base.path().trim_end_matches('/');
        let under = base_path.is_empty()
            || target.path() == base_path
            || target.path().starts_with(&format!("{base_path}/"));
        if !under {
            return Err(ConnectorError::Config(format!(
                "href `{href}` is outside the configured calendar collection"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Connector for CalendarConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Calendar
    }

    fn mode(&self) -> ConnectorMode {
        ConnectorMode::Polling {
            interval: Duration::from_secs(self.config.poll_interval_secs),
            jitter: Duration::from_secs(self.config.poll_jitter_secs),
        }
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["calendar_url", "auth"],
            "properties": {
                "calendar_url": { "type": "string", "format": "uri" },
                "auth": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["kind", "username"],
                            "properties": {
                                "kind": { "const": "app_password" },
                                "username": { "type": "string" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["kind", "token_endpoint", "client_id"],
                            "properties": {
                                "kind": { "const": "oauth" },
                                "token_endpoint": { "type": "string", "format": "uri" },
                                "client_id": { "type": "string" },
                                "client_secret": { "type": "string" },
                                "scopes": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    ]
                },
                "poll_interval_secs": { "type": "integer", "default": 900 },
                "poll_jitter_secs": { "type": "integer", "default": 60 },
                "display_name": { "type": "string" }
            }
        })
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?;
        let Some(bundle) = bundle else {
            return Ok(ConnectorAuthState::Unauthenticated);
        };
        let (auth, refreshed) = self.resolve_auth(&bundle).await?;
        if let Some(refreshed) = refreshed {
            self.persist_refreshed(&refreshed).await?;
        }
        // Probe the server with the resolved credentials (PROPFIND resourcetype).
        match CalDavClient::new(self.http.clone(), auth)
            .is_calendar(&self.config.calendar_url)
            .await
        {
            Ok(true) => Ok(ConnectorAuthState::Authenticated),
            Ok(false) => Err(ConnectorError::Config(format!(
                "configured URL is not a CalDAV calendar: {}",
                self.config.calendar_url
            ))),
            Err(ConnectorError::NotAuthenticated) => Ok(ConnectorAuthState::Expired),
            Err(e) => Err(e),
        }
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        let (client, refreshed) = match self.client_from_credentials().await {
            Ok(pair) => pair,
            Err(ConnectorError::NotAuthenticated) => return Ok(HealthStatus::NotConfigured),
            Err(ConnectorError::Authentication(_)) => return Ok(HealthStatus::AuthExpired),
            Err(e) => return Err(e),
        };
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        match client.is_calendar(&self.config.calendar_url).await {
            Ok(true) => Ok(HealthStatus::Online),
            Ok(false) => Ok(HealthStatus::Degraded),
            Err(ConnectorError::NotAuthenticated) => Ok(HealthStatus::AuthExpired),
            Err(ConnectorError::Network(_)) => Ok(HealthStatus::Offline),
            Err(e) => Err(e),
        }
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let (client, refreshed) = self.client_from_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        // Full sync ignores the persisted cursor (re-fetch everything).
        let mut token = if options.full {
            None
        } else {
            self.sync_token.lock().await.clone()
        };
        let mut fetched = 0u32;
        let mut new_cursor;
        loop {
            let result = client
                .sync_collection(&self.config.calendar_url, token.as_deref())
                .await?;
            let truncated = result.truncated;
            new_cursor = result.new_sync_token.clone();
            fetched = fetched.saturating_add(self.stage(result).await?);
            // RFC 6578 §6.5: a truncated (507) response returns a partial set
            // plus an advancing sync-token — re-request with it to page
            // through the remaining changes until the collection is drained.
            if !truncated {
                break;
            }
            token = new_cursor.clone();
            if token.is_none() {
                // No advancing token: cannot make progress, stop paging.
                break;
            }
        }
        // Advance the in-memory cursor so the next in-process cycle is
        // incremental; the supervisor persists `new_cursor` separately.
        if let Some(tok) = &new_cursor {
            *self.sync_token.lock().await = Some(tok.clone());
        }
        Ok(SyncOutcome {
            fetched,
            new_cursor,
            fetched_at: Utc::now(),
        })
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        // C4 / #198: drain the staged VEVENTs and convert each into a small
        // cluster of `NormalizedFact`s. The shared `normalize_and_insert`
        // pipeline (run by the supervisor) resolves every subject/object
        // entity via the full F5 chain, assigns connector confidence, and —
        // for the primary `user has_event <event>` fact — derives the
        // events-subsystem (#74) overlay so future-dated and recurring
        // events surface in the user's "Upcoming" memory section.
        let mut buffer = self.buffer.lock().await;
        let staged: Vec<RawCalDavEvent> = std::mem::take(&mut *buffer);
        let mut facts = Vec::new();
        for event in &staged {
            facts.extend(self.event_to_facts(event));
        }
        Ok(facts)
    }

    /// CalDAV write-back (C4 / #198): the only connector with write support.
    ///
    /// Three action kinds, each authenticated via the same credential path
    /// as `sync` (OAuth refresh included):
    /// - `create_event` — builds a VEVENT from the payload, generates a `UID`
    ///   (unless supplied), and `PUT`s it to `<calendar>/<uid>.ics` with
    ///   `If-None-Match: *`.
    /// - `update_event` — requires the target `href` (and optional `etag`),
    ///   `PUT`s with `If-Match: <etag>`.
    /// - `delete_event` — requires the target `href` (and optional `etag`),
    ///   `DELETE`s it (idempotent on 404).
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError> {
        let (client, refreshed) = self.client_from_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        match action.kind.as_str() {
            "create_event" => {
                let p: WriteEventPayload = serde_json::from_value(action.payload)
                    .map_err(|e| ConnectorError::Config(format!("create_event payload: {e}")))?;
                let uid = p
                    .uid
                    .clone()
                    .unwrap_or_else(|| format!("{}", uuid::Uuid::new_v4()));
                let href = p.href.clone().unwrap_or_else(|| {
                    format!(
                        "{}/{uid}.ics",
                        self.config.calendar_url.trim_end_matches('/')
                    )
                });
                self.ensure_in_calendar(&href)?;
                let ical = build_vevent(&p, &uid)?;
                let res = client.put_event(&href, &ical, None).await?;
                Ok(ActionResult {
                    success: true,
                    native_id: Some(res.href),
                    message: res.etag,
                })
            }
            "update_event" => {
                let p: WriteEventPayload = serde_json::from_value(action.payload)
                    .map_err(|e| ConnectorError::Config(format!("update_event payload: {e}")))?;
                let href = p
                    .href
                    .clone()
                    .ok_or_else(|| ConnectorError::Config("update_event requires `href`".into()))?;
                self.ensure_in_calendar(&href)?;
                let uid = p.uid.clone().unwrap_or_else(|| {
                    href.rsplit('/')
                        .next()
                        .map(|s| s.trim_end_matches(".ics").to_string())
                        .unwrap_or_default()
                });
                let ical = build_vevent(&p, &uid)?;
                let res = client.put_event(&href, &ical, p.etag.as_deref()).await?;
                Ok(ActionResult {
                    success: true,
                    native_id: Some(res.href),
                    message: res.etag,
                })
            }
            "delete_event" => {
                let p: DeleteEventPayload = serde_json::from_value(action.payload)
                    .map_err(|e| ConnectorError::Config(format!("delete_event payload: {e}")))?;
                let href = p.href.clone();
                self.ensure_in_calendar(&href)?;
                client.delete_event(&href, p.etag.as_deref()).await?;
                Ok(ActionResult {
                    success: true,
                    native_id: Some(href),
                    message: None,
                })
            }
            other => Err(ConnectorError::UnsupportedAction(other.to_string())),
        }
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        self.buffer.lock().await.clear();
        if let Some(store) = &self.secret_store {
            store.delete(&self.slug).await.map_err(|e| {
                ConnectorError::Authentication(format!("secret delete failed: {e}"))
            })?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Constructs [`CalendarConnector`] instances from their persisted
/// `config_json` + the shared [`SecretStore`] (Phase 3 C3 / #197).
pub struct CalendarConnectorFactory;

impl CalendarConnectorFactory {
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// Write-back payloads + builder (C4 / #198)
// ---------------------------------------------------------------------------

/// Payload for a `create_event` / `update_event` write-back action.
///
/// `start`/`end` are RFC-3339 datetimes. `attendees` are bare addresses
/// (an optional `mailto:` prefix is normalised). `uid`/`href`/`etag` apply to
/// `update_event` (and may be supplied to `create_event` to pin the id).
#[derive(Debug, Deserialize)]
struct WriteEventPayload {
    summary: String,
    start: String,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    attendees: Vec<String>,
    #[serde(default)]
    uid: Option<String>,
    #[serde(default)]
    href: Option<String>,
    #[serde(default)]
    etag: Option<String>,
}

/// Payload for a `delete_event` write-back action.
#[derive(Debug, Deserialize)]
struct DeleteEventPayload {
    href: String,
    #[serde(default)]
    etag: Option<String>,
}

/// Parse an RFC-3339 datetime into UTC, returning `None` on failure.
fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Build a `VCALENDAR`/`VEVENT` payload for a write-back `PUT`.
///
/// `uid` is the stable CalDAV item id (the href is `<calendar>/<uid>.ics`).
/// Empty optional fields are omitted so the emitted iCalendar stays minimal.
fn build_vevent(payload: &WriteEventPayload, uid: &str) -> Result<String, ConnectorError> {
    use icalendar::{Calendar, Component, Event, EventLike};
    let start = parse_rfc3339(&payload.start).ok_or_else(|| {
        ConnectorError::Config(format!("invalid `start` datetime: {}", payload.start))
    })?;
    let mut event = Event::new();
    event
        .summary(payload.summary.trim())
        .uid(uid)
        .timestamp(Utc::now())
        .starts(start);
    if let Some(end_s) = payload.end.as_deref() {
        let end = parse_rfc3339(end_s)
            .ok_or_else(|| ConnectorError::Config(format!("invalid `end` datetime: {end_s}")))?;
        event.ends(end);
    }
    if let Some(loc) = payload
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        event.location(loc);
    }
    if let Some(desc) = payload
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        event.description(desc);
    }
    for attendee in &payload.attendees {
        let mail = attendee.trim();
        if mail.is_empty() {
            continue;
        }
        let mail = mail.strip_prefix("mailto:").unwrap_or(mail);
        event.add_multi_property("ATTENDEE", &format!("mailto:{mail}"));
    }
    let event = event.done();
    let mut calendar = Calendar::new();
    calendar.push(event);
    let calendar = calendar.done();
    Ok(calendar.to_string())
}

impl Default for CalendarConnectorFactory {
    fn default() -> Self {
        Self::new()
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// The OAuth refresh / token-error unit tests that lived here were extracted
// into the shared `crate::oauth` module (Phase 3 DRY refactor) and now live in
// `src/oauth.rs`; calendar-specific behaviour is covered by the integration
// test in `tests/calendar_connector.rs`.
