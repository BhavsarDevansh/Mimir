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

/// Build a safe, persisted-error-friendly message for a failed token refresh.
///
/// Reports only the HTTP status and the parsed `error`/`error_description`
/// fields — never the raw response body, which can contain echoed request
/// parameters (the refresh token or client secret).
pub(crate) fn token_error_message(status: reqwest::StatusCode, body: &str) -> String {
    match serde_json::from_str::<TokenErrorResponse>(body) {
        Ok(TokenErrorResponse {
            error,
            error_description,
        }) => match (error, error_description) {
            (Some(err), Some(desc)) => {
                format!("token refresh failed (HTTP {status}): {err}: {desc}")
            }
            (Some(err), None) => format!("token refresh failed (HTTP {status}): {err}"),
            (None, Some(desc)) => format!("token refresh failed (HTTP {status}): {desc}"),
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
}
