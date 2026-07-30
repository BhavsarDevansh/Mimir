//! Shared OAuth 2.0 token-refresh helpers (Phase 3 DRY extract, used by the
//! Calendar connector C3 / #197 and the Email connector C5 / #199).
//!
//! The `oauth2` crate is deliberately not used here: `oauth2` 5.0 depends on
//! `reqwest` 0.12, which would duplicate the workspace's `reqwest` 0.13
//! HTTP/TLS stack. An OAuth 2.0 token refresh is a single form-encoded HTTPS
//! POST returning JSON, so it is hand-rolled on the existing `reqwest` 0.13.
//! The interactive PKCE authorization-code flow that *obtains* the first
//! token is deferred to A4 / #206.
//!
//! # Secret hygiene
//!
//! Provider error payloads routinely echo request parameters (the refresh
//! token or client secret), so the raw response body is **never** surfaced
//! to logs or persisted `last_error` strings — only the parsed
//! `error`/`error_description` fields alongside the HTTP status.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::connector::ConnectorError;
use crate::secrets::SecretBundle;

/// The `error`/`error_description` fields of an OAuth 2.0 token-error
/// response (RFC 6749 §5.2). Only these fields are surfaced to error strings
/// — the raw body is never logged or persisted, because providers may echo
/// the request's `client_secret` or `refresh_token` in error payloads.
#[derive(Deserialize)]
pub(crate) struct TokenErrorResponse {
    pub(crate) error: Option<String>,
    pub(crate) error_description: Option<String>,
}

/// Maximum length (bytes) of a provider-supplied `error_description` surfaced
/// in error strings. Provider payloads are unbounded and end up in logs and
/// the persisted `last_error`; truncating keeps the secret-hygiene promise
/// ("only parsed `error_description`, never the raw body") bounded in size.
const MAX_ERROR_DESCRIPTION_LEN: usize = 256;

/// Truncate a provider-supplied `error_description` to
/// [`MAX_ERROR_DESCRIPTION_LEN`], cutting on a UTF-8 char boundary and marking
/// the truncation with an ellipsis so a truncated value is distinguishable from
/// a complete one.
fn truncate_description(desc: &str) -> String {
    if desc.len() <= MAX_ERROR_DESCRIPTION_LEN {
        return desc.to_string();
    }
    // Walk back to the nearest UTF-8 char boundary at or below the byte limit.
    let mut end = MAX_ERROR_DESCRIPTION_LEN;
    while end > 0 && !desc.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &desc[..end])
}

/// Build a safe, persisted-error-friendly message for a failed token refresh.
///
/// Reports only the HTTP status and the parsed `error`/`error_description`
/// fields — never the raw response body, which can contain echoed request
/// parameters (the refresh token or client secret). A provider-supplied
/// `error_description` is bounded via [`truncate_description`].
pub(crate) fn token_error_message(status: reqwest::StatusCode, body: &str) -> String {
    match serde_json::from_str::<TokenErrorResponse>(body) {
        Ok(TokenErrorResponse {
            error,
            error_description,
        }) => match (error, error_description) {
            (Some(err), Some(desc)) => {
                format!(
                    "token refresh failed (HTTP {status}): {err}: {}",
                    truncate_description(&desc)
                )
            }
            (Some(err), None) => format!("token refresh failed (HTTP {status}): {err}"),
            (None, Some(desc)) => {
                format!(
                    "token refresh failed (HTTP {status}): {}",
                    truncate_description(&desc)
                )
            }
            (None, None) => format!("token refresh failed (HTTP {status})"),
        },
        Err(_) => format!("token refresh failed (HTTP {status})"),
    }
}

/// Raw JSON of an OAuth 2.0 token-refresh response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RefreshTokenResponse {
    pub(crate) access_token: Option<String>,
    /// Some providers rotate the refresh token; keep it if present, else
    /// retain the prior one.
    pub(crate) refresh_token: Option<String>,
    /// Seconds until `access_token` expires.
    pub(crate) expires_in: Option<i64>,
    #[allow(dead_code)]
    pub(crate) token_type: Option<String>,
    #[allow(dead_code)]
    pub(crate) scope: Option<String>,
}

impl RefreshTokenResponse {
    /// Build a [`SecretBundle::OAuth`] from the refresh response, retaining
    /// the prior refresh token when the response omits one.
    pub(crate) fn into_bundle(self, prior_refresh_token: Option<String>) -> SecretBundle {
        let expires_at = self
            .expires_in
            .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
        SecretBundle::OAuth {
            access_token: self.access_token.unwrap_or_default(),
            // Some providers rotate the refresh token; prefer a newly
            // returned one and retain the prior token only when the response
            // omits one (RFC 6749 §6 says a refresh_token MAY be omitted).
            refresh_token: self.refresh_token.or(prior_refresh_token),
            expires_at,
        }
    }
}

/// Whether the host of `url` is a loopback address: `localhost`, any
/// `127.0.0.0/8` IPv4 address, or `::1` IPv6.
///
/// The host is parsed as a real [`std::net::IpAddr`] so a look-alike DNS name
/// such as `127.0.0.1.evil.com` is *not* treated as loopback. `Url::host_str`
/// serialises IPv6 hosts in `[...]` bracket form (e.g. `"[::1]"`), so the
/// surrounding brackets are stripped before parsing — without this, an
/// `http://[::1]:<port>/token` loopback endpoint would be wrongly rejected.
fn is_loopback_url(url: &reqwest::Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => {
            let stripped = host
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
                .unwrap_or(host);
            stripped
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

/// Refresh an OAuth access token via a token endpoint.
///
/// `scopes`, when present, is space-joined into the request. Returns the
/// parsed refresh response for the caller to turn into a [`SecretBundle`] via
/// [`RefreshTokenResponse::into_bundle`]. Errors are sanitised so no echoed
/// request parameter reaches logs or `last_error`.
pub(crate) async fn refresh_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    scopes: Option<&[String]>,
    refresh_token: &str,
) -> Result<RefreshTokenResponse, ConnectorError> {
    // Reject non-HTTPS token endpoints before posting credentials. The refresh
    // request carries the `refresh_token` (and `client_secret`); a `http://`
    // (or otherwise typo'd) remote endpoint would leak them over an unencrypted
    // hop. Loopback HTTP (`127.0.0.1` / `::1` / `localhost`) is permitted: it is
    // Mimir's local trust boundary (same as the home-directory trust model) and
    // the credentials never traverse a network. `token_endpoint` is non-secret
    // config, but the URL parse error is surfaced as `Config` rather than
    // echoing the raw string.
    let parsed = reqwest::Url::parse(token_endpoint)
        .map_err(|e| ConnectorError::Config(format!("token endpoint is not a valid URL: {e}")))?;
    let scheme = parsed.scheme();
    let loopback = is_loopback_url(&parsed);
    if scheme != "https" && !(scheme == "http" && loopback) {
        return Err(ConnectorError::Config(format!(
            "token endpoint must use HTTPS (scheme `{scheme}` rejected); refusing to post refresh credentials over a non-loopback link"
        )));
    }

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
        ("client_id", client_id.to_string()),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    if let Some(scopes) = scopes {
        form.push(("scope", scopes.join(" ")));
    }
    let resp = http
        .post(token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| ConnectorError::Network(format!("token refresh failed: {e}")))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| ConnectorError::Network(format!("token refresh body read failed: {e}")))?;
    if !status.is_success() {
        return Err(ConnectorError::Authentication(token_error_message(
            status, &body,
        )));
    }
    let parsed: RefreshTokenResponse = serde_json::from_str(&body)
        .map_err(|e| ConnectorError::Parse(format!("token refresh JSON parse failed: {e}")))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_bundle_retains_prior_refresh_token_when_response_omits_one() {
        let resp = RefreshTokenResponse {
            access_token: Some("fresh".into()),
            refresh_token: None,
            expires_in: Some(3600),
            token_type: None,
            scope: None,
        };
        let bundle = resp.into_bundle(Some("prior-rt".into()));
        let SecretBundle::OAuth {
            access_token,
            refresh_token,
            expires_at,
        } = bundle
        else {
            panic!("expected OAuth bundle, got {bundle:?}");
        };
        assert_eq!(access_token, "fresh");
        assert_eq!(refresh_token.as_deref(), Some("prior-rt"));
        assert!(expires_at.is_some());
    }

    #[test]
    fn into_bundle_prefers_returned_refresh_token_over_prior() {
        let resp = RefreshTokenResponse {
            access_token: Some("fresh".into()),
            refresh_token: Some("rotated-rt".into()),
            expires_in: None,
            token_type: None,
            scope: None,
        };
        let bundle = resp.into_bundle(Some("prior-rt".into()));
        let SecretBundle::OAuth {
            refresh_token,
            expires_at,
            ..
        } = bundle
        else {
            panic!("expected OAuth bundle, got {bundle:?}");
        };
        assert_eq!(refresh_token.as_deref(), Some("rotated-rt"));
        assert!(expires_at.is_none());
    }

    #[test]
    fn token_error_message_reports_status_and_parsed_fields_only() {
        let body =
            r#"{"error":"invalid_grant","error_description":"the refresh token is rt-leak"}"#;
        let msg = token_error_message(reqwest::StatusCode::BAD_REQUEST, body);
        assert!(msg.contains("400"), "must mention HTTP status: {msg}");
        assert!(msg.contains("invalid_grant"), "must include error: {msg}");
        assert!(
            msg.contains("the refresh token is rt-leak"),
            "must include error_description: {msg}"
        );
        assert!(!msg.contains("BAD_REQUEST"));
    }

    #[test]
    fn token_error_message_never_echoes_raw_body_when_unparseable() {
        let body = "client_secret=leaked&refresh_token=rt-leak";
        let msg = token_error_message(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body);
        assert!(msg.contains("500"), "must mention HTTP status: {msg}");
        assert!(!msg.contains("leaked"), "raw body must not leak: {msg}");
        assert!(!msg.contains("rt-leak"), "raw body must not leak: {msg}");
    }

    #[test]
    fn truncate_description_leaves_short_text_unchanged() {
        let desc = "the refresh token is invalid";
        assert_eq!(truncate_description(desc), desc);
    }

    #[test]
    fn truncate_description_bounds_provider_supplied_text() {
        // A provider-controlled description longer than the cap is truncated to
        // <= MAX_ERROR_DESCRIPTION_LEN bytes + the ellipsis marker, on a UTF-8
        // boundary, and never leaks the full unbounded payload.
        let long = "X".repeat(MAX_ERROR_DESCRIPTION_LEN * 4);
        let truncated = truncate_description(&long);
        assert!(
            truncated.len() <= MAX_ERROR_DESCRIPTION_LEN + "…".len(),
            "truncated length {} exceeds cap + ellipsis",
            truncated.len()
        );
        assert!(
            truncated.ends_with('…'),
            "must mark truncation: {truncated}"
        );
        // The full unbounded string must not appear verbatim.
        assert_ne!(truncated, long);
    }

    #[test]
    fn truncate_description_cuts_on_a_char_boundary() {
        // Multibyte payload: the cut must land on a UTF-8 boundary (no panic, valid String).
        let desc = "é".repeat(MAX_ERROR_DESCRIPTION_LEN + 10);
        let truncated = truncate_description(&desc);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn token_error_message_truncates_a_long_error_description() {
        let long_desc = "d".repeat(1000);
        let body = format!(r#"{{"error":"invalid_grant","error_description":"{long_desc}"}}"#);
        let msg = token_error_message(reqwest::StatusCode::BAD_REQUEST, &body);
        assert!(msg.contains("invalid_grant"));
        assert!(msg.contains("400"));
        // The full 1000-char provider description must not survive into the message.
        assert!(!msg.contains(&long_desc));
        assert!(msg.ends_with('…'));
    }

    #[tokio::test]
    async fn refresh_token_rejects_non_https_endpoint() {
        // An http:// endpoint must be rejected before any credential is posted.
        let http = reqwest::Client::new();
        let err = refresh_token(
            &http,
            "http://provider.example.com/token",
            "client-id",
            None,
            None,
            "super-secret-refresh-token",
        )
        .await
        .expect_err("non-HTTPS token endpoint must be rejected");
        assert!(
            matches!(err, ConnectorError::Config(_)),
            "expected Config error, got {err:?}"
        );
        // The error must not echo the refresh token.
        let msg = format!("{err}");
        assert!(
            !msg.contains("super-secret-refresh-token"),
            "refresh token must not leak into the error: {msg}"
        );
    }

    #[tokio::test]
    async fn refresh_token_rejects_unparseable_endpoint() {
        let http = reqwest::Client::new();
        let err = refresh_token(&http, "not a url at all", "client-id", None, None, "rt")
            .await
            .expect_err("unparseable token endpoint must be rejected");
        assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
    }

    #[test]
    fn is_loopback_url_accepts_ipv6_loopback() {
        // `Url::host_str` serialises IPv6 hosts in bracket form (`"[::1]"`);
        // the brackets must be stripped before parsing or `::1` is rejected
        // even though it is a loopback address.
        assert!(is_loopback_url(
            &reqwest::Url::parse("http://[::1]:8123/token").unwrap()
        ));
        // Other loopback forms still accepted.
        assert!(is_loopback_url(
            &reqwest::Url::parse("http://127.0.0.1/token").unwrap()
        ));
        assert!(is_loopback_url(
            &reqwest::Url::parse("http://localhost/token").unwrap()
        ));
    }

    #[test]
    fn is_loopback_url_rejects_lookalike_and_remote_hosts() {
        // `127.0.0.1.evil.com` is a real DNS name, not an IP, so it must not
        // be treated as loopback even though it starts with `127.`.
        assert!(!is_loopback_url(
            &reqwest::Url::parse("http://127.0.0.1.evil.com/token").unwrap()
        ));
        // A remote host over plain HTTP is not loopback.
        assert!(!is_loopback_url(
            &reqwest::Url::parse("http://provider.example.com/token").unwrap()
        ));
        // A non-loopback IPv6 address is not loopback.
        assert!(!is_loopback_url(
            &reqwest::Url::parse("http://[2001:db8::1]/token").unwrap()
        ));
    }

    #[tokio::test]
    async fn refresh_token_accepts_ipv6_loopback_endpoint() {
        // An `http://[::1]` token endpoint is a loopback link and must pass
        // the scheme check (it is not a `Config` HTTPS-rejection). Nothing
        // listens on the port, so the request fails with a `Network` error
        // after the loopback gate — proving the endpoint was accepted.
        let http = reqwest::Client::new();
        let err = refresh_token(
            &http,
            "http://[::1]:1/token",
            "client-id",
            None,
            None,
            "super-secret-refresh-token",
        )
        .await
        .expect_err("expected a network error, not success");
        assert!(
            !matches!(err, ConnectorError::Config(_)),
            "IPv6 loopback endpoint must be accepted, got Config error: {err:?}"
        );
    }

    #[tokio::test]
    async fn refresh_token_rejects_lookalike_loopback_host() {
        // `127.0.0.1.evil.com` starts with `127.` but is a real DNS name that
        // resolves off-host; it must NOT be treated as loopback (a naive
        // prefix check would let it through and post the refresh token to a
        // remote server over plain HTTP).
        let http = reqwest::Client::new();
        let err = refresh_token(
            &http,
            "http://127.0.0.1.evil.com/token",
            "client-id",
            None,
            None,
            "super-secret-refresh-token",
        )
        .await
        .expect_err("look-alike loopback host must be rejected");
        assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
        assert!(
            !format!("{err}").contains("super-secret-refresh-token"),
            "refresh token must not leak: {err}"
        );
    }
}
