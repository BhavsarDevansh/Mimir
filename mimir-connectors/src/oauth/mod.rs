//! OAuth 2.0 client + token-refresh helpers for connector authentication
//! (Phase 3, issue #240).
//!
//! The vetted [`oauth2`] 5.0.0 protocol implementation is used with
//! `default-features = false` and a custom [`crate::oauth::OAuthHttpClient`]
//! adapter that implements the crate's `AsyncHttpClient` trait over the
//! workspace's single reqwest 0.13 client. This keeps `oauth2`'s optional
//! reqwest 0.12
//! dependency (behind its default `reqwest` feature) out of the dependency
//! tree, so the workspace keeps one HTTP/TLS stack.
//!
//! # Security properties
//!
//! - **HTTPS-only token endpoints** — `refresh_token` rejects non-HTTPS
//!   endpoints before any credential is posted, except loopback HTTP
//!   (`localhost` / `127.0.0.0/8` / `::1`), which is Mimir's local trust
//!   boundary (the same model as the home-directory secret store). The host is
//!   parsed as a real `IpAddr` so look-alike DNS names like
//!   `127.0.0.1.evil.com` are not treated as loopback.
//! - **Redirects disabled** — [`crate::oauth::OAuthHttpClient`] never follows
//!   redirects, so a malicious or compromised token endpoint cannot bounce a
//!   credential POST (RFC 6749 refresh grant) to an attacker-controlled host.
//! - **Secret hygiene** — provider error payloads routinely echo request
//!   parameters (the refresh token or client secret), so the raw response
//!   body is **never** surfaced in errors, logs, or persisted `last_error`
//!   strings. Only the parsed `error`/`error_description` fields are reported,
//!   with `error_description` truncated to 256 bytes (plus an ellipsis
//!   marker) via `MAX_ERROR_DESCRIPTION_LEN`.
//!
//! The interactive PKCE authorization-code flow that *obtains* the first
//! token is A4 / #205 and builds on [`crate::oauth::OAuthHttpClient`]; this
//! module currently exposes the refresh grant used by the Calendar and Email
//! connectors.

pub use http_client::OAuthHttpClient;
pub use pkce::{DEFAULT_FLOW_TIMEOUT, PkceFlowConfig, run_pkce_flow};

mod http_client;
pub mod pkce;
mod refresh;

/// The refresh grant is only used by the Calendar and Email backends; gate
/// the re-export to those callers (issues #351, #374) so the `oauth`-only
/// combination (e.g. the CLI PKCE flow, A4 / #205) stays warning-free.
#[cfg(any(feature = "calendar", feature = "gmail"))]
pub(crate) use refresh::resolve_access_token;
