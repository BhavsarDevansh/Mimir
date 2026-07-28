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
//! an internal buffer; [`CalendarConnector::extract`] drains the buffer and
//! returns an empty `Vec<NormalizedFact>` for now. C4 / #198 implements the
//! event → KB fact extraction + events-subsystem (#74) integration +
//! write-back (`act`).
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
    Connector, ConnectorContext, ConnectorError, ConnectorFactory, ConnectorMode, HealthStatus,
    SyncOptions, SyncOutcome,
};
use crate::secrets::{SecretBundle, SecretStore};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};

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
        Self::from_config_with_http(config, secret_store, cursor, None)
    }

    /// Build a connector, allowing an injected `http` client (tests inject a
    /// client pointed at a mock server; production passes `None` for a default
    /// 30 s-timeout client).
    pub fn from_config_with_http(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
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
            debug!(href = %href, "CalDAV reports deleted event (C4 / #198 will handle fact lifecycle)");
        }
        Ok(count)
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

    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        // C3 / #197: transport-only. Drains the staged buffer; C4 / #198 will
        // convert RawCalDavEvents into NormalizedFacts with full
        // temporal/recurrence resolution + events-subsystem integration.
        let mut buffer = self.buffer.lock().await;
        let _staged: Vec<RawCalDavEvent> = std::mem::take(&mut *buffer);
        Ok(Vec::new())
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
