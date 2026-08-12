use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default IMAP-over-TLS port (implicit TLS, RFC 8314).
pub(crate) const DEFAULT_IMAP_PORT: u16 = 993;
/// Default mailbox to sync.
pub(crate) const DEFAULT_MAILBOX: &str = "INBOX";
/// Default poll interval (5 min) for the polling fallback. IDLE servers are
/// cheap to keep open, but polling 5 min balances freshness against rate
/// limits for servers without IDLE.
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Default poll jitter (30 s).
pub(crate) const DEFAULT_POLL_JITTER: Duration = Duration::from_secs(30);
/// Default IDLE wait (28 min). RFC 2177 recommends re-issuing IDLE at least
/// every 29 min to avoid server inactivity logoff; 28 min leaves a margin.
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(28 * 60);

pub(crate) const DEFAULT_SLUG: &str = "gmail";
pub(crate) const DEFAULT_DISPLAY_NAME: &str = "Gmail";

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
    /// as a [`SecretBundle::AppPassword`](crate::secrets::SecretBundle::AppPassword); only the username is non-secret.
    AppPassword {
        /// Account username / email.
        username: String,
    },
    /// OAuth 2.0 (Gmail / Microsoft via XOAUTH2). The access/refresh tokens
    /// live in the [`SecretStore`](crate::secrets::SecretStore) as a
    /// [`SecretBundle::OAuth`](crate::secrets::SecretBundle::OAuth); only the client config + the account email
    /// are non-secret. The interactive PKCE login that obtains the first
    /// token is A4 / #205.
    #[serde(rename = "oauth")]
    OAuth {
        /// Account username / email embedded in the XOAUTH2 initial response.
        username: String,
        /// Authorization endpoint the interactive PKCE login (A4 / #205)
        /// points the user's browser at. Optional on stored records so
        /// configs persisted before the field existed (pre-0.97.0) still
        /// load; the interactive flow requires it and fails with a clear
        /// message when it is absent.
        #[serde(default)]
        auth_uri: Option<String>,
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
    pub(crate) fn discriminant(&self) -> &'static str {
        match self {
            Self::AppPassword { .. } => "app_password",
            Self::OAuth { .. } => "oauth",
        }
    }
}

/// Deserialisable configuration for [`EmailConnector`](crate::email::connector::EmailConnector), stored as the
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
pub(crate) fn encode_cursor(uid_validity: u32, last_uid: u32) -> String {
    format!("{uid_validity}:{last_uid}")
}

/// Parse an encoded cursor. Returns `None` for an empty/malformed cursor
/// (treated as "no prior cursor" → a full first sync).
pub(crate) fn parse_cursor(cursor: &str) -> Option<(u32, u32)> {
    let (v, u) = cursor.split_once(':')?;
    Some((v.parse().ok()?, u.parse().ok()?))
}

#[cfg(test)]
#[path = "config_tests.rs"]
pub(super) mod config_tests;
