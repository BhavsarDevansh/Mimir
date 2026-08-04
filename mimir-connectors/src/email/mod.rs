//! IMAP email connector (Phase 3 C5 / #199), gated by the `gmail` feature.
//!
//! An [`crate::email::imap`] transport (IMAP `LOGIN` / `AUTHENTICATE XOAUTH2`,
//! `UID FETCH` incremental sync, `IDLE` push) backs an [`EmailConnector`]
//! implementing the two-step ingestion model ([`crate::Connector`]) in either
//! `Push` (IDLE) or `Polling` (fallback) mode. Auth is an app password or an
//! OAuth access token refreshed by the connector; the interactive PKCE login
//! that *obtains* the first OAuth token is A4 / #206.
//!
//! # C5 / C6 boundary
//!
//! C5 (#199) delivers the *transport* + `sync` that stages raw RFC 822
//! messages in an internal buffer; [`EmailConnector::extract`] drains the
//! buffer and returns an empty `Vec<NormalizedFact>` for now. C6 / #200
//! implements the mail parsing + structured fact extraction
//! (headers/dates/contacts, then LLM extraction for flights/bookings in C7).
//!
//! # Mode — IDLE vs polling
//!
//! `mode` defaults to `auto`: [`Connector::authenticate`] runs a `CAPABILITY`
//! probe (which it does anyway to validate the credentials) and caches
//! whether the server advertises `IDLE`. [`Connector::mode`] then returns
//! `Push` when `IDLE` is advertised and `Polling` otherwise — a true
//! automatic polling fallback. `idle` / `poll` config values force one mode.
//!
//! # Cursor — UIDVALIDITY-safe last-UID
//!
//! The persisted cursor encodes `UIDVALIDITY:last_uid` (e.g. `"17:42"`). On
//! `SELECT`, if the mailbox's `UIDVALIDITY` differs from the cursor's, every
//! prior UID is stale (the mailbox was recreated) and the connector performs
//! a full re-fetch — UIDs alone are not a safe cursor (a plain last-UID would
//! silently miss or duplicate mail after a UIDVALIDITY bump).
//!
//! # Credentials
//!
//! Per the [`crate::secrets`] design, the non-secret auth *method* + username
//! / OAuth client config live in `config_json`; the secret itself (app
//! password or OAuth token bundle) lives in the shared
//! [`SecretStore`](crate::secrets::SecretStore) under the connector slug.

pub mod imap;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use mail_parser::{MessageParser, MimeHeaders};
use serde::{Deserialize, Serialize};
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::connector::{
    Connector, ConnectorContext, ConnectorError, ConnectorFactory, ConnectorMode, HealthStatus,
    SyncOptions, SyncOutcome,
};
use crate::email::imap::{FetchResult, ImapAuth, ImapSession, connect_tls, imap_login};
use crate::oauth;
use crate::secrets::{SecretBundle, SecretStore};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default IMAP-over-TLS port (implicit TLS, RFC 8314).
const DEFAULT_IMAP_PORT: u16 = 993;
/// Default mailbox to sync.
const DEFAULT_MAILBOX: &str = "INBOX";
/// Default poll interval (5 min) for the polling fallback. IDLE servers are
/// cheap to keep open, but polling 5 min balances freshness against rate
/// limits for servers without IDLE.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Default poll jitter (30 s).
const DEFAULT_POLL_JITTER: Duration = Duration::from_secs(30);
/// Default IDLE wait (28 min). RFC 2177 recommends re-issuing IDLE at least
/// every 29 min to avoid server inactivity logoff; 28 min leaves a margin.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(28 * 60);

const DEFAULT_SLUG: &str = "gmail";
const DEFAULT_DISPLAY_NAME: &str = "Gmail";

fn default_poll_interval_secs() -> u64 {
    DEFAULT_POLL_INTERVAL.as_secs()
}
fn default_poll_jitter_secs() -> u64 {
    DEFAULT_POLL_JITTER.as_secs()
}
fn default_idle_timeout_secs() -> u64 {
    DEFAULT_IDLE_TIMEOUT.as_secs()
}

// ---------------------------------------------------------------------------
// Config DTO (serde boundary for `config_json`)
// ---------------------------------------------------------------------------

/// How the connector should run: IDLE push, polling, or auto-detect (default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmailSyncMode {
    /// Use `IDLE` if the server advertises it, else fall back to polling.
    #[default]
    Auto,
    /// Force `IDLE` push (error if the server lacks the capability).
    Idle,
    /// Force polling (never use IDLE).
    Poll,
}

/// The non-secret auth method + parameters, stored in `config_json`. Tagged
/// by `kind` so the on-disk JSON is self-describing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmailAuthMethod {
    /// App-specific password (Gmail "app passwords", iCloud, Outlook). The
    /// password itself lives in the [`SecretStore`](crate::secrets::SecretStore)
    /// as a [`SecretBundle::AppPassword`]; only the username is non-secret.
    AppPassword {
        /// Account username / email.
        username: String,
    },
    /// OAuth 2.0 (Gmail / Microsoft via XOAUTH2). The access/refresh tokens
    /// live in the [`SecretStore`](crate::secrets::SecretStore) as a
    /// [`SecretBundle::OAuth`]; only the client config + the account email
    /// are non-secret. The interactive PKCE login that obtains the first
    /// token is A4 / #206.
    #[serde(rename = "oauth")]
    OAuth {
        /// Account username / email embedded in the XOAUTH2 initial response.
        username: String,
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

impl EmailAuthMethod {
    /// The non-secret discriminant name (the serde `kind` tag), for error
    /// messages that must not `Debug`-format the OAuth `client_secret`.
    fn discriminant(&self) -> &'static str {
        match self {
            Self::AppPassword { .. } => "app_password",
            Self::OAuth { .. } => "oauth",
        }
    }
}

/// Deserialisable configuration for [`EmailConnector`], stored as the
/// `config_json` of a `connectors` row (with `__slug` / `__ctype` /
/// `__instance_id` / `__cursor` injected by the supervisor). Unknown fields —
/// including the injected identity/cursor keys — are ignored by serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailConfigDto {
    /// IMAP server hostname (e.g. `imap.gmail.com`). Required.
    pub host: String,
    /// IMAP-over-TLS port. Defaults to 993.
    #[serde(default)]
    pub port: Option<u16>,
    /// Mailbox to sync. Defaults to `INBOX`.
    #[serde(default)]
    pub mailbox: Option<String>,
    /// Non-secret auth method + parameters. Required.
    pub auth: EmailAuthMethod,
    /// Sync mode. Defaults to `auto` (IDLE if advertised, else polling).
    #[serde(default)]
    pub mode: EmailSyncMode,
    /// Poll interval in seconds (polling fallback). Defaults to 300.
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// Poll jitter in seconds. Defaults to 30.
    #[serde(default = "default_poll_jitter_secs")]
    pub poll_jitter_secs: u64,
    /// IDLE wait in seconds (re-issue IDLE before the ~29 min server logoff).
    /// Defaults to 1680 (28 min).
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// Display name override. Defaults to "Gmail".
    #[serde(default)]
    pub display_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Cursor (UIDVALIDITY-safe last-UID)
// ---------------------------------------------------------------------------

/// Encoded incremental cursor: `<uid_validity>:<last_uid>`.
fn encode_cursor(uid_validity: u32, last_uid: u32) -> String {
    format!("{uid_validity}:{last_uid}")
}

/// Parse an encoded cursor. Returns `None` for an empty/malformed cursor
/// (treated as "no prior cursor" → a full first sync).
fn parse_cursor(cursor: &str) -> Option<(u32, u32)> {
    let (v, u) = cursor.split_once(':')?;
    Some((v.parse().ok()?, u.parse().ok()?))
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// An IMAP email connector (Phase 3 C5 / #199).
///
/// `Push`-mode (IDLE) when the server advertises `IDLE`, `Polling`-mode
/// otherwise; syncs a single mailbox by incremental UID and stages raw RFC 822
/// messages in an internal buffer.
pub struct EmailConnector {
    slug: String,
    display_name: String,
    config: EmailConfigDto,
    /// Shared credential store (loaded by slug); `None` means the daemon did
    /// not wire one in (sync/authenticate then fail `NotAuthenticated`).
    secret_store: Option<Arc<dyn SecretStore>>,
    /// Shared HTTP client for OAuth token refresh.
    http: reqwest::Client,
    /// In-memory incremental cursor (`(uid_validity, last_uid)`). Seeded from
    /// `__cursor` at construction; the supervisor persists the value returned
    /// by [`sync`](Connector::sync) via `update_sync_cursor`.
    last_uid: Mutex<Option<(u32, u32)>>,
    /// Cached `IDLE` capability, set by [`authenticate`](Connector::authenticate).
    /// `None` until the first successful capability probe. A
    /// `std::sync::Mutex` (never held across an `await`) so the
    /// sync [`mode`](Connector::mode) can read it without `try_lock`.
    supports_idle: StdMutex<Option<bool>>,
    /// Staged raw RFC 822 messages awaiting extraction (drained by `extract`).
    buffer: Mutex<Vec<imap::RawEmail>>,
    /// Canonical user identity name (the `config.toml` `[identity] name`),
    /// injected via [`ConnectorContext::user_identity`] so connector-sourced
    /// facts that are user-scoped — iMIP invite extraction's
    /// `user has_event <event>` (C6 / #200) — author against the same entity
    /// the daemon treats as `user_entity_id`. `None` when no identity is
    /// configured (the primary `has_event` fact is then skipped, matching the
    /// Calendar connector's behaviour).
    user_identity: Option<String>,
}

impl EmailConnector {
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
    /// client pointed at a mock token endpoint; production passes `None` for a
    /// default 30 s-timeout client).
    pub fn from_config_with_http(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
        http: Option<reqwest::Client>,
    ) -> Result<Self, ConnectorError> {
        // Recover the supervisor-injected slug before parsing the DTO: serde
        // ignores unknown fields (the DTO has no `deny_unknown_fields`), so
        // the injected `__slug` / `__cursor` keys pass through harmlessly.
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());
        let dto: EmailConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid email config: {e}")))?;
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
            last_uid: Mutex::new(cursor.as_deref().and_then(parse_cursor)),
            supports_idle: StdMutex::new(None),
            buffer: Mutex::new(Vec::new()),
            user_identity: user_identity
                .filter(|n| !n.trim().is_empty())
                .map(|n| n.trim().to_string()),
        })
    }

    fn port(&self) -> u16 {
        self.config.port.unwrap_or(DEFAULT_IMAP_PORT)
    }
    fn mailbox(&self) -> &str {
        self.config.mailbox.as_deref().unwrap_or(DEFAULT_MAILBOX)
    }
    fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.config.idle_timeout_secs)
    }

    /// Decide whether this cycle uses IDLE (Push) or polling (Polling).
    /// `Ok(true)` → IDLE. Honours the explicit config mode, falling back to
    /// the cached capability for `auto`. `Idle` mode errors if the server is
    /// known to lack `IDLE`, matching the documented contract. Synchronous
    /// (a `std::sync::Mutex` guard never held across an `await`).
    fn use_idle(&self) -> Result<bool, ConnectorError> {
        match self.config.mode {
            // Forced IDLE: error if the capability probe confirmed the server
            // does not advertise `IDLE`. An unprobed (`None`) cache lets the
            // IDLE attempt proceed; the server's BAD response surfaces the
            // mismatch on the next cycle.
            EmailSyncMode::Idle => match *self.supports_idle.lock().unwrap() {
                Some(false) => Err(ConnectorError::Config(
                    "idle mode requested but the server does not advertise IDLE".into(),
                )),
                _ => Ok(true),
            },
            EmailSyncMode::Poll => Ok(false),
            // `Auto` defaults to Push when unprobed, matching `mode()` (which
            // reports `ConnectorMode::Push` for `None`); otherwise follows the
            // cached capability so `mode()` and `use_idle()` never disagree
            // (a mismatch would let the supervisor's push loop busy-spin).
            EmailSyncMode::Auto => Ok(self.supports_idle.lock().unwrap().unwrap_or(true)),
        }
    }

    /// Load the secret bundle and turn it into live [`ImapAuth`] credentials,
    /// refreshing an expired OAuth access token (persisting the new bundle).
    /// Returns the auth and whether a refresh happened.
    async fn resolve_credentials(
        &self,
    ) -> Result<(ImapAuth, Option<SecretBundle>), ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?
            .ok_or(ConnectorError::NotAuthenticated)?;
        self.resolve_auth(&bundle).await
    }

    /// Turn a [`SecretBundle`] into [`ImapAuth`], refreshing an expired OAuth
    /// access token when needed. Returns the auth and the refreshed bundle (if
    /// a refresh happened) for the caller to persist.
    async fn resolve_auth(
        &self,
        bundle: &SecretBundle,
    ) -> Result<(ImapAuth, Option<SecretBundle>), ConnectorError> {
        match (&self.config.auth, bundle) {
            (EmailAuthMethod::AppPassword { username }, SecretBundle::AppPassword { password }) => {
                Ok((
                    ImapAuth::Login {
                        username: username.clone(),
                        password: password.clone(),
                    },
                    None,
                ))
            }
            (
                EmailAuthMethod::OAuth { .. },
                SecretBundle::OAuth {
                    access_token,
                    refresh_token,
                    expires_at,
                },
            ) => {
                // Refresh if expired (or within 60 s of expiry). An unknown
                // expiry (`None`) does not force a refresh every cycle — the
                // token is reused and the server's 401 triggers re-auth next.
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
                    let bundle = refreshed.into_bundle(Some(refresh_token));
                    let auth = ImapAuth::Xoauth2 {
                        username: self.oauth_username().to_string(),
                        access_token: token,
                    };
                    Ok((auth, Some(bundle)))
                } else {
                    Ok((
                        ImapAuth::Xoauth2 {
                            username: self.oauth_username().to_string(),
                            access_token: access_token.clone(),
                        },
                        None,
                    ))
                }
            }
            _ => Err(ConnectorError::Authentication(format!(
                "auth method {} does not match stored secret kind",
                self.config.auth.discriminant()
            ))),
        }
    }

    /// The OAuth account username (panics if the configured auth is not OAuth
    /// — only called from within the `OAuth` arm of [`resolve_auth`]).
    fn oauth_username(&self) -> &str {
        match &self.config.auth {
            EmailAuthMethod::OAuth { username, .. } => username,
            _ => unreachable!("oauth_username called for a non-OAuth email connector"),
        }
    }

    /// Refresh an OAuth access token via the configured token endpoint,
    /// delegating to the shared [`crate::oauth::refresh_token`] helper (DRY
    /// with the Calendar connector).
    async fn refresh_oauth(
        &self,
        refresh_token: &str,
    ) -> Result<oauth::RefreshTokenResponse, ConnectorError> {
        let EmailAuthMethod::OAuth {
            token_endpoint,
            client_id,
            client_secret,
            scopes,
            ..
        } = &self.config.auth
        else {
            return Err(ConnectorError::Config(
                "refresh_oauth called for a non-OAuth connector".into(),
            ));
        };
        oauth::refresh_token(
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

    /// Open an authenticated session to the configured IMAP server, resolving
    /// credentials (and persisting any OAuth refresh) first.
    async fn open_session(&self) -> Result<ImapSession<imap::TlsStream>, ConnectorError> {
        let (auth, refreshed) = self.resolve_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        let stream = connect_tls(&self.config.host, self.port()).await?;
        let client = async_imap::Client::new(stream);
        imap_login(client, auth).await
    }

    /// Run one sync cycle against an already-authenticated session. Generic
    /// over the stream so tests drive it against a fake server. Selects the
    /// mailbox, validates the UIDVALIDITY cursor, optionally blocks on IDLE
    /// (Push), then incrementally `UID FETCH`es and stages new messages.
    async fn run_sync<S: imap::ImapStream>(
        &self,
        mut session: ImapSession<S>,
        options: SyncOptions,
    ) -> Result<SyncOutcome, ConnectorError> {
        let idle = self.use_idle()?;
        let info = session.examine(self.mailbox()).await?;
        let uid_validity = info.uid_validity;

        // Validate the persisted cursor against the current UIDVALIDITY:
        // a mismatch (mailbox recreated) invalidates all prior UIDs → full.
        let cursor = *self.last_uid.lock().await;
        let last_uid = match (cursor, options.full) {
            (_, true) => None,
            (Some((v, u)), false) if v == uid_validity => Some(u),
            _ => None,
        };
        if cursor.is_some() && matches!(cursor, Some((v, _)) if v != uid_validity) {
            warn!(
                uid_validity,
                "IMAP UIDVALIDITY changed; performing full re-sync"
            );
        }

        let mut session = if idle {
            let (sess, new_data) = session.idle_wait(self.idle_timeout()).await?;
            if !new_data {
                // IDLE timed out / was interrupted with no new mail.
                sess.logout().await;
                return Ok(SyncOutcome {
                    fetched: 0,
                    new_cursor: None,
                    fetched_at: Utc::now(),
                });
            }
            sess
        } else {
            session
        };

        let FetchResult { messages, max_uid } = session.fetch_since(last_uid, uid_validity).await?;
        session.logout().await;

        let fetched = u32::try_from(messages.len()).unwrap_or(u32::MAX);
        self.buffer.lock().await.extend(messages);

        // Advance the cursor: persist on a full/first sync or when new mail
        // arrived; leave it unchanged when an incremental cycle fetched
        // nothing (the supervisor skips a no-op cursor write).
        let new_cursor = match (last_uid, max_uid) {
            (None, _) => Some(encode_cursor(uid_validity, max_uid)),
            (Some(prev), max) if max > prev => Some(encode_cursor(uid_validity, max)),
            _ => None,
        };
        *self.last_uid.lock().await = Some((uid_validity, max_uid));

        debug!(
            fetched,
            uid_validity,
            last_uid = max_uid,
            idle,
            "email sync cycle complete"
        );

        Ok(SyncOutcome {
            fetched,
            new_cursor,
            fetched_at: Utc::now(),
        })
    }

    /// Shared probe used by both [`Connector::authenticate`] and
    /// [`Connector::health`]: resolve credentials (refreshing OAuth),
    /// connect, log in, probe `CAPABILITY` (caching IDLE support), and log
    /// out. Returns the cached `IDLE` capability. Callers map the
    /// [`ConnectorError`] onto their respective lifecycle enums.
    async fn probe_capability(&self) -> Result<bool, ConnectorError> {
        let (auth, refreshed) = self.resolve_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        let stream = connect_tls(&self.config.host, self.port()).await?;
        let client = async_imap::Client::new(stream);
        let mut session = imap_login(client, auth).await?;
        let supports = match session.supports_idle().await {
            Ok(supports) => supports,
            Err(e) => {
                session.logout().await;
                return Err(e);
            }
        };
        session.logout().await;
        *self.supports_idle.lock().unwrap() = Some(supports);
        Ok(supports)
    }

    /// Layer 1 of the extraction cascade (C6 / #200): iMIP calendar invites.
    ///
    /// Walks every MIME part of the message for `text/calendar` parts whose
    /// iMIP `METHOD` is `REQUEST` (a meeting request) or `REPLY` (an attendee
    /// response), parses each embedded VEVENT via the shared
    /// [`crate::ical::parse_ical_to_vevents`], and turns it into the appointment
    /// fact cluster via [`crate::ical::vevent_to_facts`]. The full `parts` walk
    /// (not only `attachments()`) catches a `text/calendar` part nested inside
    /// `multipart/alternative` that carries no `Content-Disposition: attachment`
    /// header and is therefore classified as a body part by `mail-parser`.
    /// `PUBLISH` (often marketing webinars) and `CANCEL` (deletion lifecycle)
    /// are skipped for now — `CANCEL` → KB fact lifecycle is tracked
    /// separately. Every fact is provenanced with `raw_ref` (the email's
    /// `UIDVALIDITY`-qualified IMAP UID) and authored against the injected
    /// [`user_identity`](Self::user_identity) when set.
    fn extract_invites(
        &self,
        message: &mail_parser::Message<'_>,
        raw_ref: &str,
    ) -> Vec<mimir_knowledge::normalize::NormalizedFact> {
        let mut facts = Vec::new();
        for part in &message.parts {
            if !part.is_content_type("text", "calendar") {
                continue;
            }
            let Some(ct) = part.content_type() else {
                continue;
            };
            let Some(ical) = part.text_contents() else {
                debug!(
                    raw_ref,
                    "text/calendar part had no decodable text contents; skipping"
                );
                continue;
            };
            // RFC 6047 §2.4: the MIME `method` parameter is optional. Prefer it
            // when present (it must agree with the body), otherwise fall back to
            // the iCalendar `METHOD` property in the body. Property names are
            // case-insensitive (RFC 5545).
            // If both are present and disagree, the part is rejected — a
            // conflicting `METHOD` is not a valid iMIP object (RFC 6047 §2.4
            // requires the parameter and body to match when both are supplied).
            let mime_method = ct
                .attribute("method")
                .map(|m| m.trim().to_ascii_uppercase());
            let calendar_method = ical
                .lines()
                .find_map(|l| {
                    let l = l.trim();
                    l.get(..7)
                        .filter(|p| p.eq_ignore_ascii_case("METHOD:"))
                        .map(|_| l[7..].trim())
                })
                .map(str::to_ascii_uppercase);
            let method = match (mime_method, calendar_method) {
                (Some(mime), Some(calendar)) if mime != calendar => {
                    debug!(
                        raw_ref,
                        mime_method = %mime,
                        calendar_method = %calendar,
                        "skipping text/calendar part: conflicting METHOD values"
                    );
                    continue;
                }
                (Some(mime), _) => Some(mime),
                (None, Some(calendar)) => Some(calendar),
                (None, None) => None,
            };
            match method.as_deref() {
                Some("REQUEST") | Some("REPLY") => {}
                other => {
                    debug!(raw_ref, method = ?other, "skipping text/calendar part: unsupported/absent METHOD");
                    continue;
                }
            }
            for vevent in crate::ical::parse_ical_to_vevents(ical) {
                facts.extend(crate::ical::vevent_to_facts(
                    self.user_identity.as_deref(),
                    &vevent,
                    raw_ref,
                ));
            }
        }
        facts
    }
}

#[async_trait]
impl Connector for EmailConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Gmail
    }

    fn mode(&self) -> ConnectorMode {
        match self.config.mode {
            EmailSyncMode::Idle => ConnectorMode::Push,
            EmailSyncMode::Poll => ConnectorMode::Polling {
                interval: Duration::from_secs(self.config.poll_interval_secs),
                jitter: Duration::from_secs(self.config.poll_jitter_secs),
            },
            EmailSyncMode::Auto => match *self.supports_idle.lock().unwrap() {
                // `mode()` is a sync method called after `authenticate()`, so
                // the cached capability is set; `None` (not yet probed) defaults
                // to Push — IDLE is preferred and ubiquitous for the targeted
                // providers (Gmail / Outlook / iCloud).
                Some(false) => ConnectorMode::Polling {
                    interval: Duration::from_secs(self.config.poll_interval_secs),
                    jitter: Duration::from_secs(self.config.poll_jitter_secs),
                },
                _ => ConnectorMode::Push,
            },
        }
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["host", "auth"],
            "properties": {
                "host": { "type": "string" },
                "port": { "type": "integer", "default": 993 },
                "mailbox": { "type": "string", "default": "INBOX" },
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
                            "required": ["kind", "username", "token_endpoint", "client_id"],
                            "properties": {
                                "kind": { "const": "oauth" },
                                "username": { "type": "string" },
                                "token_endpoint": { "type": "string", "format": "uri" },
                                "client_id": { "type": "string" },
                                "client_secret": { "type": "string" },
                                "scopes": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    ]
                },
                "mode": { "type": "string", "enum": ["auto", "idle", "poll"], "default": "auto" },
                "poll_interval_secs": { "type": "integer", "default": 300 },
                "poll_jitter_secs": { "type": "integer", "default": 30 },
                "idle_timeout_secs": { "type": "integer", "default": 1680 },
                "display_name": { "type": "string" }
            }
        })
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        match self.probe_capability().await {
            Ok(_) => Ok(ConnectorAuthState::Authenticated),
            Err(ConnectorError::NotAuthenticated) => Ok(ConnectorAuthState::Unauthenticated),
            Err(ConnectorError::Authentication(_)) => Ok(ConnectorAuthState::Expired),
            Err(e) => Err(e),
        }
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        match self.probe_capability().await {
            Ok(_) => Ok(HealthStatus::Online),
            Err(ConnectorError::NotAuthenticated) => Ok(HealthStatus::NotConfigured),
            Err(ConnectorError::Authentication(_)) => Ok(HealthStatus::AuthExpired),
            Err(ConnectorError::Network(_)) => Ok(HealthStatus::Offline),
            Err(e) => Err(e),
        }
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let session = self.open_session().await?;
        self.run_sync(session, options).await
    }

    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        // C6 / #200: drain the staged RFC 822 messages and run a deterministic
        // (structured-parse) extraction *cascade* over each. Today the cascade
        // has one layer — iMIP calendar invites (`text/calendar; method=REQUEST
        // | REPLY`) parsed into the same VEVENT fact cluster the Calendar
        // connector emits, via the shared [`crate::ical`] module. Plain prose
        // emails (no `text/calendar` part) produce no facts: the email is
        // *provenance*, not the fact — the fact is about the real-world thing
        // the email conveys (an appointment), and prose confirmations, flights,
        // bookings, and bank statements are read by the LLM layer in C7 / #201
        // (and, for machine-readable `schema.org` JSON-LD, by #249). No
        // per-email communication facts are emitted and no `Person` entities
        // are auto-created from `From`/`To` headers, so marketing/spam produces
        // no junk facts.
        // Drain the staged buffer and release the mutex guard before the
        // CPU-bound MIME parse loop: holding the lock across parsing would block
        // a concurrent `sync()` cycle from staging new mail for the whole parse.
        let staged: Vec<imap::RawEmail> = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };
        let mut facts = Vec::new();
        for mail in &staged {
            // An IMAP UID is unique only within one mailbox + `UIDVALIDITY`
            // epoch, so qualify the provenance reference as `{uid_validity}:{uid}`
            // (matching the persisted cursor format) to stay globally unique.
            let raw_ref = format!("{}:{}", mail.uid_validity, mail.uid);
            if let Some(message) = MessageParser::default().parse(&mail.raw) {
                facts.extend(self.extract_invites(&message, &raw_ref));
            } else {
                debug!(uid = mail.uid, "could not parse RFC 822 message; skipping");
            }
        }
        Ok(facts)
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        self.buffer.lock().await.clear();
        *self.last_uid.lock().await = None;
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

/// Constructs [`EmailConnector`] instances from their persisted `config_json`
/// + the shared [`SecretStore`] (Phase 3 C5 / #199).
pub struct EmailConnectorFactory;

impl EmailConnectorFactory {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmailConnectorFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectorFactory for EmailConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError> {
        let cursor = config
            .get("__cursor")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let connector = EmailConnector::from_config_with_http(
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
// Pure-logic unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn app_config() -> serde_json::Value {
        serde_json::json!({
            "host": "imap.example.com",
            "auth": { "kind": "app_password", "username": "devansh@example.com" },
            "__slug": "gmail-personal",
            "__cursor": "17:42",
        })
    }

    #[test]
    fn cursor_round_trip() {
        assert_eq!(encode_cursor(17, 42), "17:42");
        assert_eq!(parse_cursor("17:42"), Some((17, 42)));
        assert_eq!(parse_cursor("0:0"), Some((0, 0)));
        assert_eq!(parse_cursor(""), None);
        assert_eq!(parse_cursor("17"), None);
        assert_eq!(parse_cursor("abc:def"), None);
        assert_eq!(parse_cursor("17:42:9"), None);
    }

    #[test]
    fn from_config_seeds_cursor_and_slug() {
        // The factory extracts `__cursor` from config and passes it as the
        // `cursor` param (mirroring the Calendar connector / supervisor).
        let connector =
            EmailConnector::from_config(app_config(), None, Some("17:42".into())).expect("config");
        assert_eq!(connector.id(), "gmail-personal");
        assert_eq!(connector.connector_type(), ConnectorType::Gmail);
        assert_eq!(connector.name(), "Gmail");
        assert_eq!(connector.port(), 993);
        assert_eq!(connector.mailbox(), "INBOX");
        assert_eq!(*connector.last_uid.try_lock().unwrap(), Some((17, 42)));
        // Auto mode with no capability probe yet → Push (IDLE preferred).
        assert!(matches!(connector.mode(), ConnectorMode::Push));
    }

    #[test]
    fn from_config_poll_mode_returns_polling() {
        let mut cfg = app_config();
        cfg["mode"] = serde_json::json!("poll");
        cfg["poll_interval_secs"] = 120.into();
        cfg["poll_jitter_secs"] = 10.into();
        let connector = EmailConnector::from_config(cfg, None, None).expect("config");
        assert!(matches!(
            connector.mode(),
            ConnectorMode::Polling { interval, jitter } if interval == Duration::from_secs(120)
                && jitter == Duration::from_secs(10)
        ));
    }

    #[test]
    fn from_config_explicit_idle_mode_is_push() {
        let mut cfg = app_config();
        cfg["mode"] = serde_json::json!("idle");
        let connector = EmailConnector::from_config(cfg, None, None).expect("config");
        assert!(matches!(connector.mode(), ConnectorMode::Push));
    }

    #[test]
    fn from_config_custom_port_and_mailbox() {
        let mut cfg = app_config();
        cfg["port"] = 143.into();
        cfg["mailbox"] = "[Gmail]/All Mail".into();
        let connector = EmailConnector::from_config(cfg, None, None).expect("config");
        assert_eq!(connector.port(), 143);
        assert_eq!(connector.mailbox(), "[Gmail]/All Mail");
    }

    #[test]
    fn from_config_rejects_bad_config() {
        let bad = serde_json::json!({ "host": "imap.example.com" }); // missing auth
        assert!(matches!(
            EmailConnector::from_config(bad, None, None),
            Err(ConnectorError::Config(_))
        ));
    }

    #[test]
    fn auto_mode_falls_back_to_polling_when_idle_not_advertised() {
        let connector = EmailConnector::from_config(app_config(), None, None).expect("config");
        // Simulate the capability probe caching `false`.
        *connector.supports_idle.lock().unwrap() = Some(false);
        assert!(matches!(connector.mode(), ConnectorMode::Polling { .. }));
    }

    #[tokio::test]
    async fn auth_method_mismatch_is_an_error() {
        // App-password config but an OAuth bundle stored.
        let connector = EmailConnector::from_config(app_config(), None, None).expect("config");
        let bundle = SecretBundle::OAuth {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: None,
        };
        assert!(matches!(
            connector.resolve_auth(&bundle).await,
            Err(ConnectorError::Authentication(_))
        ));
    }

    #[tokio::test]
    async fn resolve_auth_app_password_builds_login() {
        let connector = EmailConnector::from_config(app_config(), None, None).expect("config");
        let bundle = SecretBundle::AppPassword {
            password: "hunter2".into(),
        };
        let (auth, refreshed) = connector.resolve_auth(&bundle).await.expect("ok");
        assert!(refreshed.is_none());
        match auth {
            ImapAuth::Login { username, password } => {
                assert_eq!(username, "devansh@example.com");
                assert_eq!(password, "hunter2");
            }
            other => panic!("expected Login, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_auth_oauth_reuses_unexpired_token() {
        let mut cfg = app_config();
        cfg["auth"] = serde_json::json!({
            "kind": "oauth",
            "username": "devansh@example.com",
            "token_endpoint": "https://oauth.example.com/token",
            "client_id": "cid",
        });
        let connector = EmailConnector::from_config(cfg, None, None).expect("config");
        let bundle = SecretBundle::OAuth {
            access_token: "ya29.access".into(),
            refresh_token: Some("rt".into()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        let (auth, refreshed) = connector.resolve_auth(&bundle).await.expect("ok");
        assert!(refreshed.is_none(), "no refresh expected for a live token");
        match auth {
            ImapAuth::Xoauth2 {
                username,
                access_token,
            } => {
                assert_eq!(username, "devansh@example.com");
                assert_eq!(access_token, "ya29.access");
            }
            other => panic!("expected Xoauth2, got {other:?}"),
        }
    }

    // --- C6 / #200: iMIP invite extraction (cascade layer 1) -----------------

    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::enums::EventType;
    use mimir_knowledge::normalize::NormalizedFact;

    fn connector_with_identity(name: Option<&str>) -> EmailConnector {
        EmailConnector::from_config_with_http(
            app_config(),
            None,
            name.map(|n| n.to_string()),
            None,
            None,
        )
        .expect("config")
    }

    /// Build a minimal RFC 822 email carrying one `text/calendar; method=<m>`
    /// attachment whose body is a single VEVENT (a dentist appointment with a
    /// location and two attendees). The plain-text body is included so the
    /// calendar part is a real attachment, not the message body.
    pub(super) fn invite_email(method: &str) -> Vec<u8> {
        format!(
            r#"From: dentist@example.com
To: devansh@example.com
Subject: Dentist appointment
Date: Sat, 20 Nov 2025 14:22:01 -0800
Message-ID: <invite-1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="bnd"

--bnd
Content-Type: text/plain; charset="utf-8"

You are invited.
--bnd
Content-Type: text/calendar; method={method}; charset="utf-8"
Content-Disposition: attachment; filename="invite.ics"

BEGIN:VCALENDAR
VERSION:2.0
METHOD:{method}
BEGIN:VEVENT
UID:dentist-1@example.com
SUMMARY:Dentist appointment
DTSTART:20991120T140000Z
DTEND:20991120T150000Z
LOCATION:123 Main St
ATTENDEE;CN=Devansh:mailto:devansh@example.com
ATTENDEE;CN=Dr Smith:mailto:smith@dental.com
END:VEVENT
END:VCALENDAR
--bnd--
"#,
            method = method
        )
        // RFC 5322/MIME require CRLF line endings and an IMAP `BODY.PEEK[]`
        // fetch returns CRLF, so normalise the bare-LF raw string to CRLF
        // rather than relying on the parser's leniency.
        .replace('\n', "\r\n")
        .into_bytes()
    }

    pub(super) fn plain_email() -> Vec<u8> {
        b"From: marketing@retailer.com\r\n\
To: devansh@example.com\r\n\
Subject: 20% off everything\r\n\
Date: Sat, 20 Nov 2025 14:22:01 -0800\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Sale! Sale! Sale!\r\n"
            .to_vec()
    }

    fn parse(bytes: &[u8]) -> mail_parser::Message<'_> {
        mail_parser::MessageParser::default()
            .parse(bytes)
            .expect("fixture must parse")
    }

    #[test]
    fn extract_invites_emits_appointment_cluster_for_request_method() {
        let connector = connector_with_identity(Some("Devansh"));
        let bytes = invite_email("REQUEST");
        let message = parse(&bytes);
        let facts = connector.extract_invites(&message, "42");
        // 1 primary (has_event) + 1 location + 2 attendees = 4.
        assert_eq!(facts.len(), 4);
        let primary = facts
            .iter()
            .find(|f| f.relationship_type == "has_event")
            .unwrap();
        assert_eq!(primary.subject, "Devansh");
        assert_eq!(primary.subject_type, EntityType::Person);
        assert_eq!(primary.object, "Dentist appointment");
        assert_eq!(primary.object_type, Some(EntityType::Event));
        assert!(primary.valid_from.is_some());
        assert!(primary.valid_until.is_some());
        assert_eq!(primary.event_type, Some(EventType::Appointment));
        assert_eq!(primary.raw_reference.as_deref(), Some("42"));
        // Location fact carries no temporal bounds.
        let loc = facts
            .iter()
            .find(|f| f.relationship_type == "located_in")
            .unwrap();
        assert_eq!(loc.object, "123 Main St");
        assert_eq!(loc.object_type, Some(EntityType::Place));
        assert!(loc.valid_from.is_none());
        assert!(loc.valid_until.is_none());
        // Two attendee facts, no temporal bounds.
        let attendees: Vec<&NormalizedFact> = facts
            .iter()
            .filter(|f| f.relationship_type == "attending")
            .collect();
        assert_eq!(attendees.len(), 2);
        assert!(attendees.iter().all(|a| a.valid_from.is_none()));
        assert_eq!(attendees[0].subject, "Devansh");
        assert_eq!(attendees[1].subject, "Dr Smith");
    }

    #[test]
    fn extract_invites_emits_facts_for_reply_method() {
        let connector = connector_with_identity(Some("Devansh"));
        let bytes = invite_email("REPLY");
        let message = parse(&bytes);
        let facts = connector.extract_invites(&message, "7");
        // Same cluster shape as REQUEST.
        assert!(facts.iter().any(|f| f.relationship_type == "has_event"));
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.relationship_type == "attending")
                .count(),
            2
        );
    }

    #[test]
    fn extract_invites_skips_publish_method() {
        let connector = connector_with_identity(Some("Devansh"));
        let bytes = invite_email("PUBLISH");
        let message = parse(&bytes);
        assert!(
            connector.extract_invites(&message, "9").is_empty(),
            "PUBLISH (often marketing webinars) is skipped for now"
        );
    }

    #[test]
    fn extract_invites_skips_cancel_method() {
        let connector = connector_with_identity(Some("Devansh"));
        let bytes = invite_email("CANCEL");
        let message = parse(&bytes);
        assert!(
            connector.extract_invites(&message, "9").is_empty(),
            "CANCEL lifecycle is tracked separately; no facts here"
        );
    }

    /// Like [`invite_email`] but lets the MIME `method` parameter and the
    /// iCalendar body `METHOD` property diverge, so conflicting and
    /// single-source combinations can be exercised independently.
    fn invite_email_split_method(mime_method: Option<&str>, body_method: Option<&str>) -> Vec<u8> {
        let ct = match mime_method {
            Some(m) => format!("text/calendar; method={m}; charset=\"utf-8\""),
            None => "text/calendar; charset=\"utf-8\"".to_string(),
        };
        let cal_method_line = body_method
            .map(|m| format!("METHOD:{m}\r\n"))
            .unwrap_or_default();
        format!(
            r#"From: dentist@example.com
To: devansh@example.com
Subject: Dentist appointment
Date: Sat, 20 Nov 2025 14:22:01 -0800
Message-ID: <invite-1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="bnd"

--bnd
Content-Type: text/plain; charset="utf-8"

You are invited.
--bnd
Content-Type: {ct}
Content-Disposition: attachment; filename="invite.ics"

BEGIN:VCALENDAR
VERSION:2.0
{cal_method_line}BEGIN:VEVENT
UID:dentist-1@example.com
SUMMARY:Dentist appointment
DTSTART:20991120T140000Z
DTEND:20991120T150000Z
LOCATION:123 Main St
ATTENDEE;CN=Devansh:mailto:devansh@example.com
ATTENDEE;CN=Dr Smith:mailto:smith@dental.com
END:VEVENT
END:VCALENDAR
--bnd--
"#,
            ct = ct,
            cal_method_line = cal_method_line
        )
        .replace('\n', "\r\n")
        .into_bytes()
    }

    #[test]
    fn extract_invites_skips_conflicting_mime_and_body_method() {
        let connector = connector_with_identity(Some("Devansh"));
        // MIME says REQUEST, body says CANCEL — must be rejected, not silently
        // honoured as REQUEST (which would create appointment facts).
        let bytes = invite_email_split_method(Some("REQUEST"), Some("CANCEL"));
        let message = parse(&bytes);
        assert!(
            connector.extract_invites(&message, "9").is_empty(),
            "a part whose MIME `method` and body `METHOD` disagree is not a valid iMIP object"
        );
    }

    #[test]
    fn extract_invites_skips_conflicting_supported_and_unsupported_method() {
        let connector = connector_with_identity(Some("Devansh"));
        // MIME says REPLY (supported), body says PUBLISH (unsupported) — the
        // conflict is rejected before the supported/unsupported filter runs.
        let bytes = invite_email_split_method(Some("REPLY"), Some("PUBLISH"));
        let message = parse(&bytes);
        assert!(
            connector.extract_invites(&message, "9").is_empty(),
            "conflicting METHOD values are rejected regardless of which side is supported"
        );
    }

    #[test]
    fn extract_invites_falls_back_to_body_method_when_mime_absent() {
        let connector = connector_with_identity(Some("Devansh"));
        // No MIME `method` parameter; the body `METHOD:REQUEST` is the source.
        let bytes = invite_email_split_method(None, Some("REQUEST"));
        let message = parse(&bytes);
        let facts = connector.extract_invites(&message, "7");
        assert!(
            facts.iter().any(|f| f.relationship_type == "has_event"),
            "the iCalendar body `METHOD` property is used when the MIME parameter is absent"
        );
    }

    #[test]
    fn extract_invites_skips_when_neither_method_source_present() {
        let connector = connector_with_identity(Some("Devansh"));
        let bytes = invite_email_split_method(None, None);
        let message = parse(&bytes);
        assert!(
            connector.extract_invites(&message, "9").is_empty(),
            "no METHOD from either source → unsupported/absent → skipped"
        );
    }

    #[test]
    fn extract_invites_skips_plain_email() {
        let connector = connector_with_identity(Some("Devansh"));
        let bytes = plain_email();
        let message = parse(&bytes);
        // No text/calendar part → no facts. A marketing email produces nothing.
        assert!(connector.extract_invites(&message, "1").is_empty());
    }

    #[test]
    fn extract_invites_skips_primary_when_no_user_identity() {
        let connector = connector_with_identity(None);
        let bytes = invite_email("REQUEST");
        let message = parse(&bytes);
        let facts = connector.extract_invites(&message, "42");
        // No user identity → no primary has_event; the event is still captured
        // via its location + attendee facts.
        assert!(facts.iter().all(|f| f.relationship_type != "has_event"));
        assert!(facts.iter().any(|f| f.relationship_type == "located_in"));
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.relationship_type == "attending")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn extract_drains_buffer_and_returns_invite_facts() {
        let connector = connector_with_identity(Some("Devansh"));
        // Stage a raw invite email in the buffer (as a sync cycle would).
        connector.buffer.lock().await.push(imap::RawEmail {
            uid: 42,
            uid_validity: 1,
            internal_date: None,
            raw: invite_email("REQUEST"),
        });
        let facts = connector.extract().await.expect("extract");
        // The cascade produced the appointment cluster; the buffer is drained.
        assert!(facts.iter().any(|f| f.relationship_type == "has_event"));
        assert!(
            connector.buffer.lock().await.is_empty(),
            "buffer must be drained"
        );
    }

    #[tokio::test]
    async fn extract_with_no_invite_emails_yields_no_facts() {
        let connector = connector_with_identity(Some("Devansh"));
        connector.buffer.lock().await.push(imap::RawEmail {
            uid: 1,
            uid_validity: 1,
            internal_date: None,
            raw: plain_email(),
        });
        let facts = connector.extract().await.expect("extract");
        assert!(facts.is_empty(), "plain/marketing email → no facts");
    }

    // --- C6 / #200: KB integration (F5 resolution, temporal bounds, provenance)

    use mimir_knowledge::KnowledgeGraph;
    use mimir_knowledge::models::connector::UpsertConnectorInput;
    use mimir_knowledge::models::enums::ConnectorStatus;
    use mimir_knowledge::models::source::ExtractionMethod;
    use mimir_knowledge::normalize::Provenance;
    use mimir_knowledge::normalize::normalize_and_insert;

    async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
            .await
            .unwrap();
        (kg, dir)
    }

    async fn entity_named(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
        kg.search_entities(name, 10)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.entity.name == name)
            .map(|r| r.entity.id)
    }

    #[tokio::test]
    async fn extract_funnels_into_kb_with_resolution_and_provenance() {
        let (kg, _dir) = init_kg().await;
        // Register a Gmail connector instance so connector provenance has a
        // valid `connector_instance_id` FK.
        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Gmail,
                slug: "gmail-personal".to_string(),
                backend: "imap".to_string(),
                display_name: "Gmail".to_string(),
                config_json: "{}".to_string(),
                status: Some(ConnectorStatus::Active),
                auth_state: Some(ConnectorAuthState::Authenticated),
            })
            .await
            .unwrap();
        let instance_id = row.id;

        let connector = connector_with_identity(Some("Devansh"));
        connector.buffer.lock().await.push(imap::RawEmail {
            uid: 42,
            uid_validity: 123,
            internal_date: None,
            raw: invite_email("REQUEST"),
        });
        let facts = connector.extract().await.expect("extract");
        assert_eq!(facts.len(), 4);

        let outcome = normalize_and_insert(
            &kg,
            facts,
            Provenance::connector(
                instance_id,
                ConnectorType::Gmail,
                ExtractionMethod::StructuredParse,
            ),
        )
        .await
        .expect("normalize_and_insert");
        assert!(
            outcome.pending_confirmation.is_empty(),
            "no sensitive facts"
        );
        assert_eq!(outcome.inserted.len(), 4, "all four facts inserted");

        // F5: the user, the event, the place, and both attendees resolved to
        // entities (the user identity + attendees → Person; the venue → Place;
        // the SUMMARY → Event).
        let devansh = entity_named(&kg, "Devansh").await.expect("Devansh entity");
        let event = entity_named(&kg, "Dentist appointment")
            .await
            .expect("event entity");
        let place = entity_named(&kg, "123 Main St")
            .await
            .expect("place entity");
        let dr_smith = entity_named(&kg, "Dr Smith")
            .await
            .expect("Dr Smith entity");
        // The `located_in` fact resolved the venue to the Place entity.
        let mut located_in = None;
        for f in &outcome.inserted {
            if kg
                .relationship_type_name(f.relationship_type_id)
                .await
                .as_deref()
                == Some("located_in")
            {
                located_in = Some(f);
            }
        }
        let located_in = located_in.expect("located_in fact");
        assert_eq!(located_in.object_id, Some(place));

        // Locate the primary `has_event` fact and assert its temporal bounds +
        // Appointment overlay.
        let mut has_event = None;
        for f in &outcome.inserted {
            if kg
                .relationship_type_name(f.relationship_type_id)
                .await
                .as_deref()
                == Some("has_event")
            {
                has_event = Some(f);
            }
        }
        let has_event = has_event.expect("has_event fact");
        assert_eq!(has_event.subject_id, devansh);
        assert_eq!(has_event.object_id, Some(event));
        assert!(
            has_event.valid_from.is_some(),
            "DTSTART carried as valid_from"
        );
        assert!(
            has_event.valid_until.is_some(),
            "DTEND carried as valid_until"
        );
        let overlay = kg
            .get_event_by_fact(has_event.id)
            .await
            .unwrap()
            .expect("events-subsystem overlay");
        assert_eq!(overlay.event_type(), Some(EventType::Appointment));

        // Secondary facts carry no temporal bounds (no overlay) — guard against
        // the PR #248 regression where secondary facts inherited DTSTART/DTEND.
        for f in &outcome.inserted {
            let name = kg.relationship_type_name(f.relationship_type_id).await;
            if matches!(name.as_deref(), Some("located_in") | Some("attending")) {
                assert!(
                    f.valid_from.is_none(),
                    "secondary fact {} has valid_from",
                    f.id
                );
                assert!(
                    f.valid_until.is_none(),
                    "secondary fact {} has valid_until",
                    f.id
                );
                assert!(
                    kg.get_event_by_fact(f.id).await.unwrap().is_none(),
                    "secondary fact {} must not spawn an overlay",
                    f.id
                );
            }
        }
        // The attendee facts resolved both the user and Dr Smith to Person
        // entities (the user is also an attendee of their own appointment).
        // The `attending` fact whose subject is Dr Smith must have resolved to
        // the Dr Smith Person entity and point at the appointment Event.
        let mut dr_smith_attending = None;
        for f in &outcome.inserted {
            if kg
                .relationship_type_name(f.relationship_type_id)
                .await
                .as_deref()
                == Some("attending")
                && f.subject_id == dr_smith
            {
                dr_smith_attending = Some(f);
                break;
            }
        }
        let dr_smith_attending = dr_smith_attending.expect("Dr Smith attending fact");
        assert_eq!(dr_smith_attending.object_id, Some(event));

        // Provenance: every fact has one Connector source tied to the instance,
        // with the `UIDVALIDITY`-qualified UID as `raw_reference` and
        // StructuredParse method.
        for f in &outcome.inserted {
            let sources = kg.get_sources_for_fact(f.id).await.unwrap();
            assert!(
                sources.iter().any(|s| {
                    s.source_type_id
                        == mimir_knowledge::models::source::SourceType::Connector as i16
                        && s.connector_instance_id == Some(instance_id)
                        && s.connector_type_id == Some(ConnectorType::Gmail as i16)
                        && s.raw_reference.as_deref() == Some("123:42")
                        && s.extraction_method_id == Some(ExtractionMethod::StructuredParse as i16)
                }),
                "missing connector provenance on fact {}: {:?}",
                f.id,
                sources
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Fake-IMAP integration tests (login, IDLE, polling, incremental, cursor)
// ---------------------------------------------------------------------------
//
// The transport is exercised end-to-end against a minimal scripted IMAP
// server speaking the protocol over a `tokio::io::duplex` pair — no TLS, no
// live account. `Client::new` accepts any tokio async stream, so the same
// `imap_login` / `ImapSession` / `run_sync` code paths the daemon runs against
// a rustls socket run here against the fake server.

#[cfg(test)]
mod imap_integration {
    use super::*;
    use crate::email::imap::{ImapAuth, ImapSession, imap_login};
    use async_imap::Client;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Configuration for the fake IMAP server.
    struct FakeCfg {
        uid_validity: u32,
        supports_idle: bool,
        /// (uid, body) pairs the server exposes via `UID FETCH`.
        messages: Vec<(u32, Vec<u8>)>,
        /// `EXISTS` count to push during IDLE (signals new mail). `None` → IDLE
        /// times out with no push (connector returns fetched:0).
        idle_push_exists: Option<u32>,
        /// Second UIDVALIDITY returned on a *second* `SELECT` (UIDVALIDITY
        /// reset test). `None` → always returns `uid_validity`.
        second_uid_validity: Option<u32>,
        /// Omit the `UIDVALIDITY` response code on SELECT/EXAMINE to exercise
        /// the missing-UIDVALIDITY error path. `false` by default.
        omit_uid_validity: bool,
    }

    impl Default for FakeCfg {
        fn default() -> Self {
            Self {
                uid_validity: 17,
                supports_idle: true,
                messages: Vec::new(),
                idle_push_exists: None,
                second_uid_validity: None,
                omit_uid_validity: false,
            }
        }
    }

    /// Drive a fake IMAP server over `stream`. Handles exactly the verbs the
    /// connector issues (greeting, LOGIN/AUTHENTICATE, SELECT, UID FETCH, IDLE,
    /// LOGOUT). Captures the decoded XOAUTH2 SASL response into `capture` when
    /// supplied.
    async fn run_fake(
        stream: tokio::io::DuplexStream,
        cfg: FakeCfg,
        capture: Option<Arc<Mutex<Vec<u8>>>>,
        select_count: Arc<Mutex<u32>>,
    ) {
        use base64::Engine as _;
        let (read, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        writer
            .write_all(b"* OK fake IMAP ready\r\n")
            .await
            .expect("greeting");
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let line = line.trim_end_matches(['\r', '\n']).to_string();
            let mut parts = line.split_whitespace();
            let tag = parts.next().unwrap_or("").to_string();
            let verb = parts.next().unwrap_or("").to_ascii_uppercase();
            match verb.as_str() {
                "CAPABILITY" => {
                    let cap = if cfg.supports_idle {
                        "IMAP4rev1 IDLE"
                    } else {
                        "IMAP4rev1"
                    };
                    writer
                        .write_all(
                            format!("* CAPABILITY {cap}\r\n{tag} OK CAPABILITY\r\n").as_bytes(),
                        )
                        .await
                        .unwrap();
                }
                "LOGIN" => {
                    writer
                        .write_all(format!("{tag} OK LOGIN completed\r\n").as_bytes())
                        .await
                        .unwrap();
                }
                "AUTHENTICATE" => {
                    // XOAUTH2: send an empty continuation challenge.
                    writer.write_all(b"+ \r\n").await.unwrap();
                    let mut resp = String::new();
                    reader.read_line(&mut resp).await.unwrap();
                    if let Some(cap) = &capture {
                        let trimmed = resp.trim_end_matches(['\r', '\n']);
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(trimmed)
                        {
                            *cap.lock().unwrap() = bytes;
                        }
                    }
                    writer
                        .write_all(format!("{tag} OK AUTHENTICATE completed\r\n").as_bytes())
                        .await
                        .unwrap();
                }
                "SELECT" | "EXAMINE" => {
                    // Compute UIDVALIDITY (per-SELECT for the reset test) and
                    // drop the guard before awaiting so the future stays Send.
                    let uv = {
                        let mut n = select_count.lock().unwrap();
                        *n += 1;
                        if *n >= 2 {
                            cfg.second_uid_validity.unwrap_or(cfg.uid_validity)
                        } else {
                            cfg.uid_validity
                        }
                    };
                    let exists = cfg.messages.len() as u32;
                    let next = cfg.messages.iter().map(|(u, _)| *u).max().unwrap_or(0) + 1;
                    let uidvalidity_line = if cfg.omit_uid_validity {
                        String::new()
                    } else {
                        format!("* OK [UIDVALIDITY {uv}]\r\n")
                    };
                    writer
                        .write_all(
                            format!(
                                "* FLAGS (\\Seen)\r\n* {exists} EXISTS\r\n{uidvalidity_line}* OK [UIDNEXT {next}]\r\n{tag} OK [READ-WRITE] SELECT completed\r\n",
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                }
                "UID" => {
                    // `UID FETCH <range> (UID INTERNALDATE BODY.PEEK[])`
                    let sub = parts.next().unwrap_or("").to_ascii_uppercase();
                    if sub != "FETCH" {
                        writer
                            .write_all(format!("{tag} BAD unknown UID sub\r\n").as_bytes())
                            .await
                            .unwrap();
                        continue;
                    }
                    let range = parts.next().unwrap_or("1:*");
                    let start: u32 = range
                        .split(':')
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1);
                    let mut matching: Vec<&(u32, Vec<u8>)> =
                        cfg.messages.iter().filter(|(u, _)| *u >= start).collect();
                    if matching.is_empty() {
                        // `*` overlap: start > max → server returns the last.
                        if let Some(last) = cfg.messages.last() {
                            matching.push(last);
                        }
                    }
                    let mut seq = 0u32;
                    for (uid, body) in &matching {
                        seq += 1;
                        let len = body.len();
                        writer
                            .write_all(
                                format!("* {seq} FETCH (UID {uid} BODY[] {{{len}}}\r\n").as_bytes(),
                            )
                            .await
                            .unwrap();
                        writer.write_all(body).await.unwrap();
                        writer.write_all(b")\r\n").await.unwrap();
                    }
                    writer
                        .write_all(format!("{tag} OK FETCH completed\r\n").as_bytes())
                        .await
                        .unwrap();
                }
                "IDLE" => {
                    writer.write_all(b"+ idling\r\n").await.unwrap();
                    if let Some(exists) = cfg.idle_push_exists {
                        // Push new mail, then await the client's DONE.
                        writer
                            .write_all(format!("* {exists} EXISTS\r\n").as_bytes())
                            .await
                            .unwrap();
                        // Read the DONE line (untagged).
                        let mut done = String::new();
                        reader.read_line(&mut done).await.unwrap();
                        assert!(done.trim().eq_ignore_ascii_case("DONE"));
                    } else {
                        // No push: the client's `idle_wait` times out on its
                        // own; when it does `done()` it sends DONE.
                        let mut done = String::new();
                        reader.read_line(&mut done).await.unwrap();
                        assert!(done.trim().eq_ignore_ascii_case("DONE"));
                    }
                    writer
                        .write_all(format!("{tag} OK IDLE terminated\r\n").as_bytes())
                        .await
                        .unwrap();
                }
                "LOGOUT" => {
                    writer
                        .write_all(format!("* BYE\r\n{tag} OK LOGOUT\r\n").as_bytes())
                        .await
                        .unwrap();
                    break;
                }
                "" => break,
                other => {
                    writer
                        .write_all(format!("{tag} OK {other} ok\r\n").as_bytes())
                        .await
                        .unwrap();
                }
            }
        }
    }

    /// Build a connector (app-password, poll mode) + a fake session wired to a
    /// fake server with `cfg`. Returns the connector and the session.
    async fn harness(cfg: FakeCfg) -> (EmailConnector, ImapSession<tokio::io::DuplexStream>) {
        let (client, server) = tokio::io::duplex(8 * 1024);
        let select_count = Arc::new(Mutex::new(0u32));
        tokio::spawn(run_fake(server, cfg, None, Arc::clone(&select_count)));
        let mut config = super::tests::app_config();
        // Poll mode so run_sync skips IDLE and fetches immediately.
        config["mode"] = serde_json::json!("poll");
        let connector = EmailConnector::from_config(config, None, None).expect("config");
        let session = imap_login(Client::new(client), app_password_auth())
            .await
            .expect("login");
        (connector, session)
    }

    fn app_password_auth() -> ImapAuth {
        ImapAuth::Login {
            username: "devansh@example.com".into(),
            password: "hunter2".into(),
        }
    }

    #[tokio::test]
    async fn polling_incremental_sync_advances_cursor() {
        let cfg = FakeCfg {
            messages: vec![
                (10u32, b"msg-10".to_vec()),
                (11, b"msg-11".to_vec()),
                (12, b"msg-12".to_vec()),
            ],
            ..Default::default()
        };
        let (connector, session) = harness(cfg).await;
        // First sync (full, no cursor): fetches all 3, cursor → 17:12.
        let outcome = connector.run_sync(session, SyncOptions::default()).await;
        let outcome = outcome.expect("sync ok");
        assert_eq!(outcome.fetched, 3);
        assert_eq!(outcome.new_cursor.as_deref(), Some("17:12"));
        assert_eq!(*connector.last_uid.lock().await, Some((17, 12)));
        let staged = connector.buffer.lock().await;
        assert_eq!(staged.len(), 3);
        assert_eq!(staged[0].uid, 10);
        assert_eq!(staged[2].uid, 12);
        assert_eq!(staged[2].raw, b"msg-12");
    }

    #[tokio::test]
    async fn polling_incremental_skips_already_synced() {
        let cfg = FakeCfg {
            messages: vec![
                (10u32, b"a".to_vec()),
                (11, b"b".to_vec()),
                (12, b"c".to_vec()),
            ],
            ..Default::default()
        };
        let (connector, session) = harness(cfg).await;
        // Seed the cursor at 11: only UID 12 should be fetched.
        *connector.last_uid.lock().await = Some((17, 11));
        let outcome = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect("sync ok");
        assert_eq!(outcome.fetched, 1);
        assert_eq!(outcome.new_cursor.as_deref(), Some("17:12"));
        let staged = connector.buffer.lock().await;
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].uid, 12);
    }

    #[tokio::test]
    async fn no_new_mail_leaves_cursor_unchanged() {
        let cfg = FakeCfg {
            messages: vec![(10u32, b"a".to_vec()), (11, b"b".to_vec())],
            ..Default::default()
        };
        let (connector, session) = harness(cfg).await;
        *connector.last_uid.lock().await = Some((17, 11));
        let outcome = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect("sync ok");
        assert_eq!(outcome.fetched, 0);
        assert!(outcome.new_cursor.is_none(), "no rewrite on a no-op cycle");
        assert_eq!(*connector.last_uid.lock().await, Some((17, 11)));
    }

    #[tokio::test]
    async fn uidvalidity_reset_triggers_full_resync() {
        // The mailbox was recreated: the server now advertises a *new*
        // UIDVALIDITY (99) on SELECT, while the persisted cursor is from the
        // old epoch (17). The connector must detect the mismatch and do a
        // full re-fetch rather than trust the stale UID cursor.
        let cfg = FakeCfg {
            uid_validity: 99,
            messages: vec![(1u32, b"fresh-1".to_vec())],
            ..Default::default()
        };
        let (connector, session) = harness(cfg).await;
        *connector.last_uid.lock().await = Some((17, 11));
        let outcome = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect("sync ok");
        assert_eq!(outcome.fetched, 1);
        assert_eq!(outcome.new_cursor.as_deref(), Some("99:1"));
        assert_eq!(*connector.last_uid.lock().await, Some((99, 1)));
    }

    #[tokio::test]
    async fn idle_push_triggers_incremental_fetch() {
        // IDLE mode: the connector blocks on IDLE until the server pushes an
        // EXISTS, then fetches the new message.
        let cfg = FakeCfg {
            messages: vec![(20u32, b"new-mail".to_vec())],
            idle_push_exists: Some(1),
            ..Default::default()
        };
        let (client, server) = tokio::io::duplex(8 * 1024);
        let select_count = Arc::new(Mutex::new(0u32));
        tokio::spawn(run_fake(server, cfg, None, Arc::clone(&select_count)));
        let mut config = super::tests::app_config();
        config["mode"] = serde_json::json!("idle");
        // Seed cursor at 17:19 so UID 20 is new.
        let connector =
            EmailConnector::from_config(config, None, Some("17:19".into())).expect("config");
        let session = imap_login(Client::new(client), app_password_auth())
            .await
            .expect("login");
        let outcome = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect("sync ok");
        assert_eq!(outcome.fetched, 1);
        assert_eq!(outcome.new_cursor.as_deref(), Some("17:20"));
        let staged = connector.buffer.lock().await;
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].raw, b"new-mail");
    }

    #[tokio::test]
    async fn idle_timeout_no_new_mail_returns_zero() {
        // IDLE with no push: the connector's idle_wait uses a short timeout in
        // tests. Configure a 1-second idle timeout.
        let cfg = FakeCfg {
            idle_push_exists: None,
            messages: vec![(5u32, b"x".to_vec())],
            ..Default::default()
        };
        let (client, server) = tokio::io::duplex(8 * 1024);
        let select_count = Arc::new(Mutex::new(0u32));
        tokio::spawn(run_fake(server, cfg, None, Arc::clone(&select_count)));
        let mut config = super::tests::app_config();
        config["mode"] = serde_json::json!("idle");
        config["idle_timeout_secs"] = 1.into();
        let connector =
            EmailConnector::from_config(config, None, Some("17:5".into())).expect("config");
        let session = imap_login(Client::new(client), app_password_auth())
            .await
            .expect("login");
        let outcome = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect("sync ok");
        assert_eq!(outcome.fetched, 0);
        assert!(outcome.new_cursor.is_none());
    }

    #[tokio::test]
    async fn xoauth2_login_sends_correct_sasl_response() {
        // Verify the XOAUTH2 SASL initial response the connector would send to
        // Gmail/Microsoft: base64("user=..\x01auth=Bearer <token>\x01\x01").
        let cfg = FakeCfg::default();
        let (client, server) = tokio::io::duplex(8 * 1024);
        let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let select_count = Arc::new(Mutex::new(0u32));
        tokio::spawn(run_fake(
            server,
            cfg,
            Some(Arc::clone(&captured)),
            Arc::clone(&select_count),
        ));
        let auth = ImapAuth::Xoauth2 {
            username: "devansh@example.com".into(),
            access_token: "ya29.token".into(),
        };
        let _session = imap_login(Client::new(client), auth)
            .await
            .expect("xoauth2 login");
        let decoded = captured.lock().unwrap().clone();
        assert_eq!(
            decoded,
            b"user=devansh@example.com\x01auth=Bearer ya29.token\x01\x01".to_vec(),
            "XOAUTH2 SASL initial response must match the spec format"
        );
    }

    #[tokio::test]
    async fn missing_uidvalidity_is_an_error_not_zero() {
        // RFC 3501 mandates the UIDVALIDITY response code. A server that omits
        // it must not collapse to epoch 0 (which collides with a persisted
        // `0:<uid>` cursor and would silently skip mail) — it must error.
        let cfg = FakeCfg {
            omit_uid_validity: true,
            messages: vec![(1u32, b"fresh-1".to_vec())],
            ..Default::default()
        };
        let (connector, session) = harness(cfg).await;
        let err = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect_err("missing UIDVALIDITY must error");
        assert!(
            matches!(err, ConnectorError::Parse(_)),
            "expected Parse error, got {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("UIDVALIDITY"),
            "error must name UIDVALIDITY: {msg}"
        );
    }

    #[tokio::test]
    async fn forced_idle_errors_when_server_lacks_idle() {
        // `Idle` mode documents "error if the server lacks the capability".
        // With the capability probe cached as Some(false), `use_idle()` must
        // return a Config error instead of silently polling.
        let mut config = super::tests::app_config();
        config["mode"] = serde_json::json!("idle");
        let connector = EmailConnector::from_config(config, None, None).expect("config");
        *connector.supports_idle.lock().unwrap() = Some(false);
        let err = connector
            .use_idle()
            .expect_err("forced IDLE on an IDLE-less server must error");
        assert!(
            matches!(err, ConnectorError::Config(_)),
            "expected Config error, got {err:?}"
        );
        assert!(
            format!("{err}").contains("IDLE"),
            "error must mention IDLE: {err}"
        );
    }

    #[test]
    fn auto_mode_defaults_to_push_when_unprobed_matching_mode() {
        // Before the first capability probe `supports_idle` is `None`.
        // `mode()` reports Push for Auto+None, so `use_idle()` must also opt
        // into IDLE (true) — a mismatch would let the supervisor's push loop
        // busy-spin on immediate polling returns.
        let mut config = super::tests::app_config();
        // Default mode is `auto`; confirm explicitly.
        config["mode"] = serde_json::json!("auto");
        let connector = EmailConnector::from_config(config, None, None).expect("config");
        assert!(connector.use_idle().expect("auto+None should be Push"));
        assert!(matches!(connector.mode(), ConnectorMode::Push));
    }

    #[tokio::test]
    async fn imap_sync_then_extract_yields_invite_facts() {
        // End-to-end over the fake IMAP transport: a real RFC 822 invite
        // fetched via `UID FETCH ... BODY.PEEK[]` is staged, then `extract()`
        // turns it into the appointment fact cluster. Proves the C5 transport
        // and C6 extraction compose without a live account.
        let invite = super::tests::invite_email("REQUEST");
        let cfg = FakeCfg {
            messages: vec![(42u32, invite)],
            ..Default::default()
        };
        let (client, server) = tokio::io::duplex(8 * 1024);
        let select_count = Arc::new(Mutex::new(0u32));
        tokio::spawn(run_fake(server, cfg, None, Arc::clone(&select_count)));
        let mut config = super::tests::app_config();
        config["mode"] = serde_json::json!("poll");
        let connector =
            EmailConnector::from_config_with_http(config, None, Some("Devansh".into()), None, None)
                .expect("config");
        let session = imap_login(Client::new(client), app_password_auth())
            .await
            .expect("login");
        let outcome = connector
            .run_sync(session, SyncOptions::default())
            .await
            .expect("sync");
        assert_eq!(outcome.fetched, 1);

        let facts = connector.extract().await.expect("extract");
        assert!(
            facts.iter().any(|f| f.relationship_type == "has_event"),
            "invite extracted into a has_event fact: {facts:?}"
        );
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.relationship_type == "attending")
                .count(),
            2
        );
        // Provenance `raw_reference` is the `UIDVALIDITY`-qualified UID
        // (`{uidvalidity}:{uid}`) of the staged message.
        assert!(
            facts
                .iter()
                .all(|f| f.raw_reference.as_deref() == Some("17:42")),
            "raw_reference must be the UIDVALIDITY-qualified UID: {facts:?}"
        );
        assert!(
            connector.buffer.lock().await.is_empty(),
            "extract drains the staged buffer"
        );
    }
}
