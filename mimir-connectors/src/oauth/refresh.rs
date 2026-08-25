//! OAuth 2.0 token-refresh helpers (RFC 6749 §6) on the shared
//! [`OAuthHttpClient`].
//!
//! Both the Calendar (C3 / #197) and Email (C5 / #199) connectors funnel
//! through [`refresh_token`] / [`resolve_access_token`], so the refresh
//! grant, the endpoint security gate, and the secret-hygiene error mapping
//! live in exactly one place.

use chrono::{DateTime, Utc};
#[cfg(any(feature = "calendar", feature = "email", test))]
use oauth2::basic::BasicClient;
use oauth2::basic::{BasicErrorResponse, BasicTokenResponse};
#[cfg(any(feature = "calendar", feature = "email", test))]
use oauth2::{AuthType, ClientId, ClientSecret, RefreshToken, Scope, TokenUrl};
use oauth2::{HttpClientError, RequestTokenError, TokenResponse};

use crate::connector::ConnectorError;
use crate::secrets::SecretBundle;

#[cfg(any(feature = "calendar", feature = "email", test))]
use super::OAuthHttpClient;

/// Refresh when the stored token is within this many seconds of expiry (or
/// past it). Only read by `resolve_access_token` (Calendar / Email) and the
/// module's unit tests, so it is cfg-gated to those callers (issues #351, #374).
#[cfg(any(feature = "calendar", feature = "email", test))]
const REFRESH_SKEW_SECS: i64 = 60;

/// Maximum length (bytes) of a provider-supplied `error_description` surfaced
/// in error strings. Provider payloads are unbounded and end up in logs and
/// the persisted `last_error`; truncating keeps the secret-hygiene promise
/// ("only parsed `error`/`error_description`, never the raw body") bounded in
/// size.
pub(crate) const MAX_ERROR_DESCRIPTION_LEN: usize = 256;

/// Truncate a provider-supplied `error_description` to
/// [`MAX_ERROR_DESCRIPTION_LEN`], cutting on a UTF-8 char boundary and marking
/// the truncation with an ellipsis so a truncated value is distinguishable
/// from a complete one.
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

/// The error type of an `oauth2` refresh-grant request over the workspace
/// reqwest 0.13 client (via [`OAuthHttpClient`]).
type RefreshGrantError =
    RequestTokenError<oauth2::HttpClientError<reqwest::Error>, BasicErrorResponse>;

/// Reject a token endpoint that is not HTTPS (or loopback HTTP) before any
/// credential is posted. The refresh request carries the `refresh_token`
/// (and `client_secret`); a `http://` (or otherwise typo'd) remote endpoint
/// would leak them over an unencrypted hop. Loopback HTTP (`127.0.0.1` /
/// `::1` / `localhost`) is permitted: it is Mimir's local trust boundary
/// (same as the home-directory trust model) and the credentials never
/// traverse a network. `token_endpoint` is non-secret config, but the URL
/// parse error is surfaced as `Config` rather than echoing the raw string.
///
/// Shared by the refresh grant and the interactive PKCE code exchange
/// (A4 / #205): both post credentials to the token endpoint.
pub(crate) fn validate_token_endpoint(token_endpoint: &str) -> Result<(), ConnectorError> {
    let parsed = reqwest::Url::parse(token_endpoint)
        .map_err(|e| ConnectorError::Config(format!("token endpoint is not a valid URL: {e}")))?;
    let scheme = parsed.scheme();
    let loopback = is_loopback_url(&parsed);
    if scheme != "https" && !(scheme == "http" && loopback) {
        return Err(ConnectorError::Config(format!(
            "token endpoint must use HTTPS (scheme `{scheme}` rejected); refusing to post credentials over a non-loopback link"
        )));
    }
    Ok(())
}

/// Map an `oauth2` refresh-grant error onto [`ConnectorError`], preserving
/// the secret-hygiene promise: the raw response body is never surfaced
/// (providers may echo the request's `client_secret` or `refresh_token` in
/// error payloads). Only the parsed `error`/`error_description` fields are
/// reported, bounded via [`truncate_description`]. `what` names the grant
/// ("token refresh" / "token exchange") so the shared mapping reads
/// correctly for both the refresh grant and the PKCE code exchange
/// (A4 / #205).
pub(crate) fn map_token_error(err: RefreshGrantError, what: &str) -> ConnectorError {
    match err {
        RequestTokenError::ServerResponse(resp) => {
            let detail = truncate_description(&resp.to_string());
            ConnectorError::Authentication(format!("{what} failed: {detail}"))
        }
        RequestTokenError::Request(e) => {
            // oauth2's `HttpClientError::Reqwest` Display is the constant
            // "client error" (the inner error is not part of the format
            // string), so format the inner error to keep the actual cause
            // (DNS / timeout / TLS / connection) in logs and `last_error`.
            // The inner reqwest error contains no secret material (URL +
            // error kind).
            let detail = match &e {
                HttpClientError::Reqwest(inner) => inner.to_string(),
                HttpClientError::Http(err) => err.to_string(),
                HttpClientError::Io(err) => err.to_string(),
                HttpClientError::Other(msg) => msg.clone(),
                _ => e.to_string(),
            };
            ConnectorError::Network(format!("{what} failed: {detail}"))
        }
        // The parse error's `Vec<u8>` payload is the raw response body —
        // deliberately dropped.
        RequestTokenError::Parse(_, _) => {
            ConnectorError::Parse(format!("{what} response parse failed"))
        }
        RequestTokenError::Other(msg) => {
            ConnectorError::Authentication(format!("{what} failed: {msg}"))
        }
    }
}

/// Upper bound on a provider-supplied `expires_in`. Saturating at `MAX_UTC`
/// would make `needs_refresh` permanently false (the connector would reuse a
/// dead access token forever), so a hostile or absurd value is clamped to a
/// plausible lifetime instead.
const MAX_TOKEN_LIFETIME_DAYS: i64 = 90;

/// `expires_in` seconds → absolute expiry, clamped to
/// [`MAX_TOKEN_LIFETIME_DAYS`] (a provider value cannot realistically exceed
/// 90 days, but a hostile value must not panic).
fn expires_at_from_now(secs: std::time::Duration) -> DateTime<Utc> {
    let cap = chrono::Duration::days(MAX_TOKEN_LIFETIME_DAYS);
    let dur = chrono::Duration::from_std(secs).unwrap_or(cap).min(cap);
    // `DateTime + TimeDelta` panics on overflow, and `Duration::MAX` (~292e9
    // years) is far beyond chrono's ±262143-year range, so a hostile
    // `expires_in` must clamp instead of panicking the refresh path.
    let now = Utc::now();
    now.checked_add_signed(dur).unwrap_or(now + cap)
}

/// Build a [`SecretBundle::OAuth`] from an `oauth2` token response, retaining
/// the prior refresh token when the response omits one (RFC 6749 §6 says a
/// `refresh_token` MAY be omitted) and carrying the client secret through so
/// it stays in the credential bundle instead of `config_json`.
pub(crate) fn into_bundle(
    response: &BasicTokenResponse,
    prior_refresh_token: Option<String>,
    client_secret: Option<String>,
) -> SecretBundle {
    SecretBundle::OAuth {
        access_token: response.access_token().secret().clone(),
        // Some providers rotate the refresh token; prefer a newly returned
        // one and retain the prior token only when the response omits one.
        refresh_token: response
            .refresh_token()
            .map(|rt| rt.secret().clone())
            .or(prior_refresh_token),
        expires_at: response.expires_in().map(expires_at_from_now),
        client_secret,
    }
}

/// Refresh an OAuth access token via a token endpoint.
///
/// `scopes`, when present, is space-joined into the request. Returns the
/// parsed `oauth2` token response for the caller to turn into a
/// [`SecretBundle`] via [`into_bundle`]. Errors are sanitised so no echoed
/// request parameter reaches logs or `last_error`.
///
/// Only called by `resolve_access_token` (Calendar / Email) and the module's
/// unit tests, so it is cfg-gated to those callers (issues #351, #374).
#[cfg(any(feature = "calendar", feature = "email", test))]
pub(crate) async fn refresh_token(
    http: &OAuthHttpClient,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    scopes: Option<&[String]>,
    refresh_token: &str,
) -> Result<BasicTokenResponse, ConnectorError> {
    validate_token_endpoint(token_endpoint)?;

    // Client credentials are sent in the request body (not HTTP Basic) to
    // match the providers Mimir targets (Google, Apple, Nextcloud), which
    // accept both; the body form matches the pre-#240 hand-rolled behaviour.
    let client = BasicClient::new(ClientId::new(client_id.to_string()))
        .set_token_uri(TokenUrl::new(token_endpoint.to_string()).map_err(|e| {
            ConnectorError::Config(format!("token endpoint is not a valid URL: {e}"))
        })?)
        .set_auth_type(AuthType::RequestBody);
    let client = match client_secret {
        Some(secret) => client.set_client_secret(ClientSecret::new(secret.to_string())),
        None => client,
    };

    let refresh = RefreshToken::new(refresh_token.to_string());
    let mut request = client.exchange_refresh_token(&refresh);
    for scope in scopes.into_iter().flatten() {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    request
        .request_async(http)
        .await
        .map_err(|e| map_token_error(e, "token refresh"))
}

/// Resolve a live OAuth access token from a stored [`SecretBundle::OAuth`],
/// refreshing via the token endpoint when the stored token is expired (or
/// within [`REFRESH_SKEW_SECS`] of expiry).
///
/// Returns the access token to use and, when a refresh happened, the
/// refreshed bundle for the caller to persist. An unknown expiry (`None`)
/// does not force a refresh on every cycle — that would triple the POSTs
/// against the token endpoint and invite rate limiting. The token is reused
/// as-is; if it is actually expired the provider returns 401 and the next
/// cycle re-authenticates.
///
/// Only called by the Calendar and Email backends and the module's unit
/// tests, so it is cfg-gated to those callers (issues #351, #374).
#[cfg(any(feature = "calendar", feature = "email", test))]
pub(crate) async fn resolve_access_token(
    http: &OAuthHttpClient,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    scopes: Option<&[String]>,
    bundle: &SecretBundle,
    force: bool,
) -> Result<(String, Option<SecretBundle>), ConnectorError> {
    let SecretBundle::OAuth {
        access_token,
        refresh_token: stored_refresh_token,
        expires_at,
        client_secret: stored_client_secret,
    } = bundle
    else {
        return Err(ConnectorError::Authentication(
            "OAuth auth method requires an OAuth secret bundle".into(),
        ));
    };
    // `force` (issue #507) bypasses the skew window: the supervisor calls the
    // forced path once when a health probe reports `AuthExpired`, so a stale
    // or transiently rejected access token is refreshed and retried instead
    // of pausing the connector.
    let needs_refresh = force
        || expires_at
            .map(|exp| exp <= Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS))
            .unwrap_or(false);
    if !needs_refresh {
        return Ok((access_token.clone(), None));
    }
    let stored_refresh_token = stored_refresh_token.clone().ok_or_else(|| {
        ConnectorError::Authentication(if force {
            "OAuth access token was rejected and no refresh token is stored".into()
        } else {
            "OAuth access token expired and no refresh token is stored".into()
        })
    })?;
    let refreshed = refresh_token(
        http,
        token_endpoint,
        client_id,
        // The bundle's client secret wins (wizard-created connectors keep it
        // out of `config_json`); the config value is the pre-bundle fallback
        // for connectors registered before the field existed.
        stored_client_secret.as_deref().or(client_secret),
        scopes,
        &stored_refresh_token,
    )
    .await?;
    let token = refreshed.access_token().secret().clone();
    Ok((
        token,
        Some(into_bundle(
            &refreshed,
            Some(stored_refresh_token),
            // Adopt the config secret into the refreshed bundle so the
            // bundle becomes authoritative for future refreshes.
            stored_client_secret
                .clone()
                .or_else(|| client_secret.map(str::to_string)),
        )),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth2::basic::BasicTokenResponse;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN_ENDPOINT: &str = "https://oauth.example.com/token";

    fn client() -> OAuthHttpClient {
        OAuthHttpClient::from_client(reqwest::Client::new())
    }

    #[test]
    fn into_bundle_retains_prior_refresh_token_when_response_omits_one() {
        let resp: BasicTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "fresh",
            "token_type": "Bearer",
            "expires_in": 3600,
        }))
        .expect("token response");
        let bundle = into_bundle(&resp, Some("prior-rt".into()), Some("client-secret".into()));
        let SecretBundle::OAuth {
            access_token,
            refresh_token,
            expires_at,
            client_secret,
        } = bundle
        else {
            panic!("expected OAuth bundle, got {bundle:?}");
        };
        assert_eq!(access_token, "fresh");
        assert_eq!(refresh_token.as_deref(), Some("prior-rt"));
        assert_eq!(client_secret.as_deref(), Some("client-secret"));
        let remaining = expires_at
            .expect("expiry")
            .signed_duration_since(Utc::now());
        assert!((3590..=3610).contains(&remaining.num_seconds()));
    }

    #[test]
    fn into_bundle_prefers_returned_refresh_token_over_prior() {
        let resp: BasicTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "fresh",
            "token_type": "Bearer",
            "refresh_token": "rotated-rt",
        }))
        .expect("token response");
        let bundle = into_bundle(&resp, Some("prior-rt".into()), None);
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

    #[tokio::test]
    async fn refresh_token_posts_refresh_grant_and_parses_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=rt-value"))
            .and(body_string_contains("client_id=cid"))
            .and(body_string_contains("client_secret=secret"))
            .and(body_string_contains("scope=read+write"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "rotated-rt",
                "scope": "read write",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let scopes = vec!["read".to_string(), "write".to_string()];
        let resp = refresh_token(
            &client(),
            &format!("{}/token", server.uri()),
            "cid",
            Some("secret"),
            Some(&scopes),
            "rt-value",
        )
        .await
        .expect("refresh");

        assert_eq!(resp.access_token().secret(), "new-access");
        assert_eq!(
            resp.refresh_token().expect("rotated").secret(),
            "rotated-rt"
        );
        let remaining = resp.expires_in().expect("expiry");
        assert!(
            remaining.as_secs() >= 3595,
            "expires_in parsed: {remaining:?}"
        );
    }

    #[tokio::test]
    async fn refresh_token_without_client_secret_omits_secret_param() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("client_id=cid"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access",
                "token_type": "Bearer",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let resp = refresh_token(
            &client(),
            &format!("{}/token", server.uri()),
            "cid",
            None,
            None,
            "rt-value",
        )
        .await
        .expect("refresh");
        assert_eq!(resp.access_token().secret(), "new-access");
    }

    #[tokio::test]
    async fn refresh_token_surfaces_only_parsed_error_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "token expired",
                "echo": "super-secret-refresh-token",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let err = refresh_token(
            &client(),
            &format!("{}/token", server.uri()),
            "cid",
            None,
            None,
            "super-secret-refresh-token",
        )
        .await
        .expect_err("400 must fail");
        assert!(
            matches!(err, ConnectorError::Authentication(_)),
            "got {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("invalid_grant"),
            "parsed error code missing: {msg}"
        );
        assert!(
            !msg.contains("super-secret-refresh-token"),
            "raw body must not leak into the error: {msg}"
        );
    }

    #[tokio::test]
    async fn map_token_error_surfaces_underlying_network_cause() {
        // oauth2's `HttpClientError::Reqwest` Display is the constant "client
        // error" (the inner reqwest error is not part of the format string),
        // so the mapping must format the inner error itself — otherwise every
        // DNS/timeout/TLS/connection failure reads as "client error" in logs
        // and the persisted `last_error`.
        let reqwest_err = reqwest::Client::new()
            .get("http://")
            .send()
            .await
            .expect_err("invalid URL must fail at send time");
        let err = map_token_error(
            RequestTokenError::Request(HttpClientError::Reqwest(Box::new(reqwest_err))),
            "token refresh",
        );
        let msg = format!("{err}");
        assert!(
            matches!(err, ConnectorError::Network(_)),
            "network failure must map to Network, got {msg}"
        );
        assert!(
            msg.len() > "token refresh failed: ".len(),
            "underlying reqwest cause missing: {msg}"
        );
        assert!(
            !msg.contains("client error"),
            "constant oauth2 display leaked instead of the cause: {msg}"
        );
    }

    #[tokio::test]
    async fn refresh_token_truncates_a_long_error_description() {
        let server = MockServer::start().await;
        let long_desc = "d".repeat(1000);
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": long_desc,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let err = refresh_token(
            &client(),
            &format!("{}/token", server.uri()),
            "cid",
            None,
            None,
            "rt",
        )
        .await
        .expect_err("400 must fail");
        let msg = format!("{err}");
        assert!(msg.contains("invalid_grant"));
        assert!(
            !msg.contains(&"d".repeat(1000)),
            "description not truncated: {msg}"
        );
        assert!(msg.ends_with('…'));
    }

    #[tokio::test]
    async fn refresh_token_does_not_follow_redirects() {
        // A redirecting token endpoint must not bounce the credential POST to
        // a second host: the adapter client is built with redirects disabled,
        // so the 302 surfaces as an error response and the attacker host sees
        // nothing. Uses the hardened `OAuthHttpClient::new()` (production
        // path) — the raw reqwest client used in other tests follows
        // redirects by default.
        let attacker = MockServer::start().await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", attacker.uri()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&attacker)
            .await;

        refresh_token(
            &OAuthHttpClient::new().expect("hardened client"),
            &format!("{}/token", server.uri()),
            "cid",
            None,
            None,
            "rt-value",
        )
        .await
        .expect_err("redirect must fail the refresh");

        let attacks = attacker.received_requests().await.expect("requests");
        assert!(
            attacks.is_empty(),
            "credential POST was redirected: {attacks:?}"
        );
    }

    #[tokio::test]
    async fn refresh_token_rejects_non_https_endpoint() {
        // An http:// endpoint must be rejected before any credential is posted.
        let err = refresh_token(
            &client(),
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
        let err = refresh_token(&client(), "not a url at all", "client-id", None, None, "rt")
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
    fn expires_at_from_now_clamps_hostile_values_to_90_days() {
        // u64::MAX seconds overflows chrono's `TimeDelta`; ~317,000 years
        // fits in a `TimeDelta` but overflows `DateTime`'s ±262143-year
        // range. Neither may panic, and neither may saturate at `MAX_UTC`
        // (that would make `needs_refresh` permanently false and the
        // connector would reuse a dead token forever) — both clamp to the
        // 90-day cap.
        for secs in [u64::MAX, 10_000_000_000_000] {
            let capped = expires_at_from_now(std::time::Duration::from_secs(secs));
            let remaining = capped.signed_duration_since(Utc::now());
            assert!(
                (chrono::Duration::days(89)..=chrono::Duration::days(90)).contains(&remaining),
                "expiry not clamped to the 90-day cap: {remaining:?}"
            );
        }
    }

    #[test]
    fn into_bundle_clamps_hostile_expires_in() {
        // A hostile token endpoint can return an absurd `expires_in`; the
        // bundle conversion must clamp instead of panicking the refresh path
        // (and must not store `MAX_UTC`, which would disable proactive
        // refresh forever).
        let resp: BasicTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "fresh",
            "token_type": "Bearer",
            "expires_in": 18446744073709551615u64,
        }))
        .expect("token response");
        let bundle = into_bundle(&resp, None, None);
        let SecretBundle::OAuth { expires_at, .. } = bundle else {
            panic!("expected OAuth bundle, got {bundle:?}");
        };
        let remaining = expires_at
            .expect("clamped expiry")
            .signed_duration_since(Utc::now());
        assert!(
            (chrono::Duration::days(89)..=chrono::Duration::days(90)).contains(&remaining),
            "expiry not clamped to the 90-day cap: {remaining:?}"
        );
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
        let err = refresh_token(
            &client(),
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
        assert!(
            !format!("{err}").contains("super-secret-refresh-token"),
            "refresh token must not leak: {err}"
        );
    }

    #[tokio::test]
    async fn refresh_token_rejects_lookalike_loopback_host() {
        // `127.0.0.1.evil.com` starts with `127.` but is a real DNS name that
        // resolves off-host; it must NOT be treated as loopback (a naive
        // prefix check would let it through and post the refresh token to a
        // remote server over plain HTTP).
        let err = refresh_token(
            &client(),
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

    #[tokio::test]
    async fn resolve_access_token_reuses_unexpired_token() {
        let bundle = SecretBundle::OAuth {
            access_token: "ya29.access".into(),
            refresh_token: Some("rt".into()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            client_secret: None,
        };
        let (token, refreshed) =
            resolve_access_token(&client(), TOKEN_ENDPOINT, "cid", None, None, &bundle, false)
                .await
                .expect("no refresh needed");
        assert_eq!(token, "ya29.access");
        assert!(refreshed.is_none());
    }

    #[tokio::test]
    async fn resolve_access_token_reuses_token_with_unknown_expiry() {
        // An unknown expiry (`expires_at: None`) must not force a refresh on
        // every cycle — the token is reused as-is; if it is actually expired
        // the provider returns 401 and the next cycle re-authenticates.
        let bundle = SecretBundle::OAuth {
            access_token: "ya29.access".into(),
            refresh_token: Some("rt".into()),
            expires_at: None,
            client_secret: None,
        };
        let (token, refreshed) =
            resolve_access_token(&client(), TOKEN_ENDPOINT, "cid", None, None, &bundle, false)
                .await
                .expect("unknown expiry must not force a refresh");
        assert_eq!(token, "ya29.access");
        assert!(refreshed.is_none());
    }

    #[tokio::test]
    async fn resolve_access_token_within_skew_refreshes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("client_secret=bundle-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh",
                "token_type": "Bearer",
            })))
            .expect(1)
            .mount(&server)
            .await;
        let bundle = SecretBundle::OAuth {
            access_token: "stale".into(),
            refresh_token: Some("rt".into()),
            // 30 s before expiry — inside the 60 s refresh skew.
            expires_at: Some(Utc::now() + chrono::Duration::seconds(30)),
            // The bundle's secret wins over the config value supplied below.
            client_secret: Some("bundle-secret".into()),
        };
        let (token, refreshed) = resolve_access_token(
            &client(),
            &format!("{}/token", server.uri()),
            "cid",
            Some("config-secret"),
            None,
            &bundle,
            false,
        )
        .await
        .expect("refresh");
        assert_eq!(token, "fresh");
        let SecretBundle::OAuth {
            refresh_token,
            client_secret,
            ..
        } = refreshed.expect("refreshed bundle")
        else {
            panic!("expected OAuth bundle");
        };
        assert_eq!(
            refresh_token.as_deref(),
            Some("rt"),
            "prior refresh token retained"
        );
        assert_eq!(
            client_secret.as_deref(),
            Some("bundle-secret"),
            "client secret must be carried through the refreshed bundle"
        );
    }

    #[tokio::test]
    async fn resolve_access_token_expired_without_refresh_token_errors() {
        let bundle = SecretBundle::OAuth {
            access_token: "stale".into(),
            refresh_token: None,
            expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
            client_secret: None,
        };
        let err =
            resolve_access_token(&client(), TOKEN_ENDPOINT, "cid", None, None, &bundle, false)
                .await
                .expect_err("expired without refresh token must fail");
        assert!(
            matches!(err, ConnectorError::Authentication(_)),
            "got {err:?}"
        );
        assert!(format!("{err}").contains("no refresh token"));
    }

    #[tokio::test]
    async fn resolve_access_token_rejects_non_oauth_bundle() {
        let bundle = SecretBundle::AppPassword {
            password: "hunter2".into(),
        };
        let err =
            resolve_access_token(&client(), TOKEN_ENDPOINT, "cid", None, None, &bundle, false)
                .await
                .expect_err("non-OAuth bundle must fail");
        assert!(
            matches!(err, ConnectorError::Authentication(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn resolve_access_token_force_refreshes_unexpired_token() {
        // Issue #507: the supervisor's forced path must refresh even when the
        // stored token is still inside its lifetime — a health probe may have
        // been rejected by the service despite a not-yet-expired token.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh",
                "token_type": "Bearer",
            })))
            .expect(1)
            .mount(&server)
            .await;
        let bundle = SecretBundle::OAuth {
            access_token: "ya29.access".into(),
            refresh_token: Some("rt".into()),
            // An hour of lifetime left — the skew path would reuse it.
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            client_secret: None,
        };
        let (token, refreshed) = resolve_access_token(
            &client(),
            &format!("{}/token", server.uri()),
            "cid",
            None,
            None,
            &bundle,
            true,
        )
        .await
        .expect("forced refresh");
        assert_eq!(token, "fresh");
        assert!(
            refreshed.is_some(),
            "a forced refresh must return a refreshed bundle to persist"
        );
    }
}
