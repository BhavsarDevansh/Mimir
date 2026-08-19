//! Interactive PKCE authorization-code flow (RFC 7636) for the CLI
//! (Phase 3 A4 / issue #205).
//!
//! The flow runs entirely in the CLI process — the daemon never runs a
//! transient HTTP server. It binds an ephemeral loopback listener on
//! `127.0.0.1:0`, opens the provider's authorize URL in the user's browser
//! (with a PKCE S256 challenge and a CSRF `state`), receives the redirect on
//! the loopback listener, validates the state, exchanges the code at the
//! token endpoint via the shared [`OAuthHttpClient`], and returns the
//! resulting [`SecretBundle::OAuth`] for the caller to POST to the daemon's
//! token-ingest route.
//!
//! # Security properties
//!
//! - **Loopback-only listener** — bound to `127.0.0.1` (never `0.0.0.0`), so
//!   no remote host can race the callback and steal the authorization code.
//! - **CSRF state validation** — the callback's `state` must match the
//!   generated [`CsrfToken`] or the flow aborts without exchanging.
//! - **Token endpoint gate** — the exchange posts the code (and optional
//!   `client_secret`) through the same HTTPS/loopback gate as the refresh
//!   grant (`super::refresh::validate_token_endpoint`).
//! - **Secret hygiene** — exchange errors go through the shared
//!   `super::refresh::map_token_error` mapping: only parsed
//!   `error`/`error_description` (truncated) are surfaced, never the raw
//!   response body.
//! - **Bounded callback read** — the loopback request is read with an 8 KiB
//!   cap, so a hostile local process cannot force a large allocation.
//! - **Per-connection read deadline** — a connection that sends nothing (or
//!   a partial request) is dropped after a short deadline instead of
//!   stalling the flow until the overall timeout, so a stalled or hostile
//!   local process cannot waste the user's authorization.
//!
//! The authorize URL is printed by the caller regardless of whether the
//! browser could be opened, so the flow works in headless/SSH sessions (the
//! user opens the URL manually and the loopback redirect still completes).

use std::collections::HashMap;
use std::time::Duration;

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    Scope, TokenUrl,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::connector::ConnectorError;
use crate::secrets::SecretBundle;

use super::http_client::OAuthHttpClient;
use super::refresh::{into_bundle, map_token_error, validate_token_endpoint};

/// Non-secret OAuth client configuration needed to run the interactive PKCE
/// authorization-code flow. Mirrors the OAuth arms of the Calendar and Email
/// auth-method DTOs (which additionally carry backend-specific fields such as
/// `username` / `calendar_url` that the flow does not need).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceFlowConfig {
    /// Authorization endpoint the user's browser is pointed at.
    pub auth_uri: String,
    /// Token endpoint the authorization code is exchanged at.
    pub token_endpoint: String,
    /// OAuth client id (public clients have no secret).
    pub client_id: String,
    /// OAuth client secret (optional for PKCE public clients).
    pub client_secret: Option<String>,
    /// Scope(s) to request, each added to the authorize URL.
    pub scopes: Option<Vec<String>>,
}

/// Default time the flow waits for the browser callback before aborting.
pub const DEFAULT_FLOW_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Per-connection deadline for reading the loopback callback request. A
/// browser sends its request immediately after connecting; a connection
/// that sends nothing (or a partial request) within this window is dropped
/// so a stalled or hostile local process cannot block the flow.
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Path the loopback listener expects the provider to redirect to.
const CALLBACK_PATH: &str = "/callback";

/// Upper bound on the loopback callback request (request line + headers).
/// A browser redirect is a few hundred bytes; anything larger is rejected
/// rather than buffered whole.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// Run the interactive PKCE authorization-code flow.
///
/// `open_browser` is called with the authorize URL (the caller decides how to
/// open it — a real browser, or a test double that drives the callback).
/// Returns the exchanged [`SecretBundle::OAuth`] (access token, refresh
/// token if issued, clamped expiry) for the caller to persist via the
/// daemon's token-ingest route.
pub async fn run_pkce_flow(
    config: &PkceFlowConfig,
    http: &OAuthHttpClient,
    open_browser: &(dyn Fn(&str) + Send + Sync),
    timeout: Duration,
) -> Result<SecretBundle, ConnectorError> {
    // Reject a non-HTTPS (non-loopback) token endpoint before any credential
    // material is exchanged — same gate as the refresh grant.
    validate_token_endpoint(&config.token_endpoint)?;

    // The authorization endpoint is where the user's credentials are
    // entered, so it must be HTTPS (RFC 8252 §7.5) — never plain HTTP, even
    // on loopback. The token endpoint gate above permits loopback HTTP
    // because that is Mimir's local trust boundary; the browser is not.
    let auth_url = url::Url::parse(&config.auth_uri)
        .map_err(|e| ConnectorError::Config(format!("auth_uri is not a valid URL: {e}")))?;
    if auth_url.scheme() != "https" {
        return Err(ConnectorError::Config(
            "auth_uri must be an https URL — the authorization endpoint carries the user's credentials"
                .to_string(),
        ));
    }

    // Bind the loopback callback listener on an ephemeral port. The redirect
    // URI is derived from the bound port, so the provider must accept
    // loopback redirect URIs (RFC 8252 native-app pattern).
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

    let client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(config.auth_uri.clone())
                .map_err(|e| ConnectorError::Config(format!("auth_uri is not a valid URL: {e}")))?,
        )
        .set_token_uri(TokenUrl::new(config.token_endpoint.clone()).map_err(|e| {
            ConnectorError::Config(format!("token endpoint is not a valid URL: {e}"))
        })?)
        .set_redirect_uri(RedirectUrl::new(redirect_uri).map_err(|e| {
            ConnectorError::Config(format!("redirect URI is not a valid URL: {e}"))
        })?);
    let client = match &config.client_secret {
        Some(secret) => client.set_client_secret(ClientSecret::new(secret.clone())),
        None => client,
    };

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut authorize = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);
    for scope in config.scopes.iter().flatten() {
        authorize = authorize.add_scope(Scope::new(scope.clone()));
    }
    let (authorize_url, csrf_token) = authorize.url();

    open_browser(authorize_url.as_str());

    let code = tokio::time::timeout(
        timeout,
        wait_for_callback(&listener, &csrf_token, CONNECTION_READ_TIMEOUT),
    )
    .await
    .map_err(|_| {
            ConnectorError::Authentication(format!(
                "authorization timed out after {}s — no callback received; the flow has aborted, so re-run the command to start a new login",
                timeout.as_secs()
            ))
        })??;

    let response = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(http)
        .await
        .map_err(|e| map_token_error(e, "token exchange"))?;

    Ok(into_bundle(&response, None, config.client_secret.clone()))
}

/// Outcome of a single loopback connection.
enum CallbackOutcome {
    /// Not a callback request (e.g. a favicon probe) — keep waiting.
    Ignore,
    /// Valid callback carrying the authorization code.
    Code(String),
}

/// Accept loopback connections until a valid callback arrives, validating
/// the CSRF state on every callback-shaped request.
async fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &CsrfToken,
    connection_timeout: Duration,
) -> Result<String, ConnectorError> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        match handle_connection(&mut socket, expected_state, connection_timeout).await? {
            CallbackOutcome::Ignore => continue,
            CallbackOutcome::Code(code) => return Ok(code),
        }
    }
}

/// Read one bounded HTTP request, parse it, respond to the browser, and
/// classify the outcome.
async fn handle_connection(
    socket: &mut TcpStream,
    expected_state: &CsrfToken,
    connection_timeout: Duration,
) -> Result<CallbackOutcome, ConnectorError> {
    let Some(request) = read_request(socket, connection_timeout).await? else {
        // Stalled connection — drop it and keep waiting for the real
        // callback.
        return Ok(CallbackOutcome::Ignore);
    };
    match parse_callback(&request, expected_state.secret()) {
        Ok(None) => {
            respond(socket, 404, "Not Found", "Not found.").await?;
            Ok(CallbackOutcome::Ignore)
        }
        Ok(Some(code)) => {
            respond(socket, 200, "OK", SUCCESS_HTML).await?;
            Ok(CallbackOutcome::Code(code))
        }
        Err(message) => {
            // The message can contain a provider-controlled `error` param —
            // never echo it into the HTML response (XSS on the loopback
            // origin). The diagnostic stays in the process error only.
            respond(
                socket,
                400,
                "Bad Request",
                "Authorization failed. Return to the terminal.",
            )
            .await?;
            Err(ConnectorError::Authentication(message))
        }
    }
}

/// Parse a loopback HTTP request into a callback outcome.
///
/// Returns `Ok(None)` for requests that are not the callback (malformed
/// requests, non-GET methods, other paths — browsers probe for favicons and
/// the like, which must not abort the flow), `Ok(Some(code))` for a valid
/// callback whose `state` matches, and `Err` for a callback-shaped request
/// that must abort the flow (provider `error` param, missing/incorrect
/// `state`, missing `code`).
fn parse_callback(request: &str, expected_state: &str) -> Result<Option<String>, String> {
    let Some((path, query)) = parse_request_line(request) else {
        return Ok(None);
    };
    if path != CALLBACK_PATH {
        return Ok(None);
    }
    let params = parse_query(query);
    if let Some(error) = params.get("error") {
        return Err(format!("authorization failed: {error}"));
    }
    let state = params
        .get("state")
        .ok_or_else(|| "callback missing state parameter".to_string())?;
    if state != expected_state {
        return Err("callback state mismatch — possible CSRF; aborting".to_string());
    }
    let code = params
        .get("code")
        .ok_or_else(|| "callback missing code parameter".to_string())?;
    Ok(Some(code.clone()))
}

/// Split the request line into `(path, query)`. Returns `None` for requests
/// that are not a plain `GET` with a target (anything else is ignored).
fn parse_request_line(request: &str) -> Option<(&str, &str)> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some((path, query))
}

/// Percent-decode the query string into key/value pairs.
fn parse_query(query: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

/// Read a single HTTP request with a hard byte cap and a per-connection
/// deadline. The read stops at the end of the header block (`\r\n\r\n`) or
/// the cap, whichever comes first — a GET callback carries no body.
///
/// Returns `Ok(None)` when the connection stalls (no complete request within
/// `deadline`); the caller drops the connection and keeps waiting, so a
/// hostile local process cannot block the flow.
async fn read_request(
    socket: &mut TcpStream,
    deadline: Duration,
) -> Result<Option<String>, ConnectorError> {
    let mut buf = [0u8; MAX_REQUEST_BYTES];
    let mut read = 0;
    let read_loop = async {
        loop {
            let n = socket.read(&mut buf[read..]).await?;
            if n == 0 {
                break;
            }
            read += n;
            if read >= MAX_REQUEST_BYTES || buf[..read].ends_with(b"\r\n\r\n") {
                break;
            }
        }
        Ok::<(), ConnectorError>(())
    };
    match tokio::time::timeout(deadline, read_loop).await {
        Err(_) => Ok(None),
        Ok(Err(e)) => Err(e),
        Ok(Ok(())) => Ok(Some(String::from_utf8_lossy(&buf[..read]).into_owned())),
    }
}

/// Write a minimal HTTP/1.1 response to the browser.
async fn respond(
    socket: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> Result<(), ConnectorError> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}

/// Page shown in the browser after a successful callback.
const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Mimir authorization</title></head><body><h1>Authorization complete</h1><p>You can close this tab and return to the terminal.</p></body></html>";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        callback_url, mount_token_endpoint, parse_authorize_url, self_callback_opener,
    };
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn flow_config(token_endpoint: &str) -> PkceFlowConfig {
        PkceFlowConfig {
            auth_uri: "https://oauth.example.com/authorize".to_string(),
            token_endpoint: token_endpoint.to_string(),
            client_id: "test-client".to_string(),
            client_secret: None,
            scopes: Some(vec!["calendar.readonly".to_string()]),
        }
    }

    fn test_http() -> OAuthHttpClient {
        OAuthHttpClient::from_client(reqwest::Client::new())
    }

    // -------------------------------------------------------------------
    // parse_callback
    // -------------------------------------------------------------------

    #[test]
    fn parse_callback_accepts_valid_callback() {
        let request =
            "GET /callback?code=abc123&state=csrf-token HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(
            parse_callback(request, "csrf-token").unwrap(),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_callback_ignores_non_callback_requests() {
        // favicon probe, non-GET method, and malformed request line must not
        // abort the flow.
        let favicon = "GET /favicon.ico HTTP/1.1\r\n\r\n";
        assert_eq!(parse_callback(favicon, "csrf-token").unwrap(), None);
        let post = "POST /callback?code=x&state=csrf-token HTTP/1.1\r\n\r\n";
        assert_eq!(parse_callback(post, "csrf-token").unwrap(), None);
        let garbage = "not a request line\r\n\r\n";
        assert_eq!(parse_callback(garbage, "csrf-token").unwrap(), None);
    }

    #[test]
    fn parse_callback_rejects_state_mismatch() {
        let request = "GET /callback?code=abc123&state=wrong HTTP/1.1\r\n\r\n";
        let err = parse_callback(request, "csrf-token").unwrap_err();
        assert!(err.contains("state mismatch"), "got: {err}");
    }

    #[test]
    fn parse_callback_rejects_missing_state_or_code() {
        let no_state = "GET /callback?code=abc123 HTTP/1.1\r\n\r\n";
        assert!(
            parse_callback(no_state, "csrf-token")
                .unwrap_err()
                .contains("state")
        );
        let no_code = "GET /callback?state=csrf-token HTTP/1.1\r\n\r\n";
        assert!(
            parse_callback(no_code, "csrf-token")
                .unwrap_err()
                .contains("code")
        );
    }

    #[test]
    fn parse_callback_rejects_provider_error_param() {
        let request = "GET /callback?error=access_denied&state=csrf-token HTTP/1.1\r\n\r\n";
        let err = parse_callback(request, "csrf-token").unwrap_err();
        assert!(err.contains("access_denied"), "got: {err}");
    }

    #[test]
    fn parse_callback_percent_decodes_query_values() {
        let request = "GET /callback?code=a%2Bb%2Fc&state=csrf%20token HTTP/1.1\r\n\r\n";
        assert_eq!(
            parse_callback(request, "csrf token").unwrap(),
            Some("a+b/c".to_string())
        );
    }

    // -------------------------------------------------------------------
    // run_pkce_flow
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn run_pkce_flow_exchanges_code_and_returns_bundle() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, 1).await;

        let bundle = run_pkce_flow(
            &flow_config(&format!("{}/token", server.uri())),
            &test_http(),
            &self_callback_opener("auth-code"),
            Duration::from_secs(10),
        )
        .await
        .expect("flow");

        let SecretBundle::OAuth {
            access_token,
            refresh_token,
            expires_at,
            client_secret,
        } = bundle
        else {
            panic!("expected OAuth bundle");
        };
        assert_eq!(access_token, "ya29.access");
        assert_eq!(refresh_token.as_deref(), Some("rt"));
        assert_eq!(client_secret.as_deref(), None);
        assert!(
            expires_at.is_some_and(|exp| exp > chrono::Utc::now()),
            "expiry must be in the future"
        );
    }

    #[tokio::test]
    async fn run_pkce_flow_carries_client_secret_into_bundle() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            // The oauth2 `BasicClient` sends the client secret via HTTP
            // Basic auth (base64 of `client_id:client_secret`).
            .and(header(
                "authorization",
                "Basic dGVzdC1jbGllbnQ6Y2xpZW50LXNlY3JldA==",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.access",
                "token_type": "Bearer",
                "refresh_token": "rt",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut config = flow_config(&format!("{}/token", server.uri()));
        config.client_secret = Some("client-secret".into());

        let bundle = run_pkce_flow(
            &config,
            &test_http(),
            &self_callback_opener("auth-code"),
            Duration::from_secs(10),
        )
        .await
        .expect("flow");
        let SecretBundle::OAuth { client_secret, .. } = bundle else {
            panic!("expected OAuth bundle");
        };
        assert_eq!(client_secret.as_deref(), Some("client-secret"));
    }

    #[tokio::test]
    async fn run_pkce_flow_aborts_on_state_mismatch_without_exchanging() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, 0).await;

        // A hostile/buggy callback with the wrong state must abort the flow
        // before any code is exchanged.
        let opener = |url: &str| {
            let url = url.to_string();
            tokio::spawn(async move {
                let params = parse_authorize_url(&url).expect("authorize URL");
                let callback = callback_url(&params.redirect_uri, "stolen", "wrong-state");
                let _ = reqwest::get(callback).await;
            });
        };

        let err = run_pkce_flow(
            &flow_config(&format!("{}/token", server.uri())),
            &test_http(),
            &opener,
            Duration::from_secs(10),
        )
        .await
        .expect_err("state mismatch must abort");
        assert!(
            matches!(err, ConnectorError::Authentication(_)),
            "got {err:?}"
        );
        assert!(err.to_string().contains("state mismatch"), "got: {err}");
    }

    #[tokio::test]
    async fn run_pkce_flow_times_out_without_callback() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, 0).await;

        let opener = |_url: &str| {}; // no browser, no callback
        let err = run_pkce_flow(
            &flow_config(&format!("{}/token", server.uri())),
            &test_http(),
            &opener,
            Duration::from_millis(100),
        )
        .await
        .expect_err("timeout must abort");
        assert!(
            matches!(err, ConnectorError::Authentication(_)),
            "got {err:?}"
        );
        assert!(err.to_string().contains("timed out"), "got: {err}");
        // The flow has aborted and the listener is closed — the message must
        // tell the user to re-run rather than complete a login that can no
        // longer complete.
        assert!(err.to_string().contains("re-run"), "got: {err}");
    }

    #[tokio::test]
    async fn read_request_times_out_on_stalled_connection() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        // A local process connects and sends nothing — the read must be
        // dropped after the per-connection deadline instead of blocking the
        // flow until the overall timeout.
        let _stalled = TcpStream::connect(addr).await.unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket, Duration::from_millis(50))
            .await
            .expect("stalled read must not error");
        assert_eq!(request, None, "stalled connection must be dropped");
    }

    #[tokio::test]
    async fn wait_for_callback_drops_stalled_connection_and_accepts_real_callback() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let csrf = CsrfToken::new("csrf-token".to_string());

        // A hostile/stalled local process connects and sends nothing. The
        // connection must be dropped after the per-connection deadline and
        // the flow must still accept the real callback.
        let _stalled = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let callback = format!("http://127.0.0.1:{port}/callback?code=abc&state=csrf-token");
        let callback_task = tokio::spawn(async move {
            let _ = reqwest::get(callback).await;
        });
        let code = wait_for_callback(&listener, &csrf, Duration::from_millis(50))
            .await
            .expect("flow must survive a stalled connection");
        callback_task.await.unwrap();
        assert_eq!(code, "abc");
    }

    #[tokio::test]
    async fn run_pkce_flow_rejects_non_https_token_endpoint() {
        let err = run_pkce_flow(
            &flow_config("http://oauth.example.com/token"),
            &test_http(),
            &|_url: &str| {},
            Duration::from_secs(10),
        )
        .await
        .expect_err("non-loopback HTTP token endpoint must be rejected");
        assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn run_pkce_flow_rejects_invalid_auth_uri() {
        let mut config = flow_config("https://oauth.example.com/token");
        config.auth_uri = "not a url".to_string();
        let err = run_pkce_flow(
            &config,
            &test_http(),
            &|_url: &str| {},
            Duration::from_secs(10),
        )
        .await
        .expect_err("invalid auth_uri must be rejected");
        assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn run_pkce_flow_rejects_non_https_auth_uri_without_opening_browser() {
        let mut config = flow_config("https://oauth.example.com/token");
        config.auth_uri = "http://oauth.example.com/authorize".to_string();
        let opened = std::sync::atomic::AtomicBool::new(false);
        let opener = |_url: &str| opened.store(true, std::sync::atomic::Ordering::SeqCst);
        let err = run_pkce_flow(&config, &test_http(), &opener, Duration::from_secs(10))
            .await
            .expect_err("non-https auth_uri must be rejected");
        assert!(matches!(err, ConnectorError::Config(_)), "got {err:?}");
        assert!(
            !opened.load(std::sync::atomic::Ordering::SeqCst),
            "browser must not be opened for a non-https authorization endpoint"
        );
    }

    #[tokio::test]
    async fn callback_error_page_does_not_echo_provider_input() {
        // A provider-controlled `error` param (here with markup) must not be
        // echoed into the HTML response — that would be XSS on the loopback
        // origin. The diagnostic stays in the process error only.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let csrf = CsrfToken::new("csrf-token".to_string());
        let callback = format!(
            "http://127.0.0.1:{port}/callback?error=access_denied%3Cscript%3E&state=csrf-token"
        );
        let callback_task = tokio::spawn(async move {
            let response = reqwest::get(callback).await.unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            (status, body)
        });
        let err = wait_for_callback(&listener, &csrf, Duration::from_secs(5))
            .await
            .expect_err("provider error must abort the flow");
        let (status, body) = callback_task.await.unwrap();
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert!(
            !body.contains("access_denied"),
            "provider error must not be echoed into HTML: {body}"
        );
        assert!(body.contains("Authorization failed"), "got: {body}");
        assert!(err.to_string().contains("access_denied"), "got: {err}");
    }

    #[tokio::test]
    async fn run_pkce_flow_ignores_favicon_probe_before_callback() {
        // A browser typically requests /favicon.ico after the callback page;
        // here it arrives *before* the real callback and must not abort.
        let server = MockServer::start().await;
        mount_token_endpoint(&server, 1).await;

        let opener = |url: &str| {
            let url = url.to_string();
            tokio::spawn(async move {
                let params = parse_authorize_url(&url).expect("authorize URL");
                // A favicon probe on the same origin, before the real
                // callback — must be ignored, not abort the flow.
                let favicon = reqwest::Url::parse(&params.redirect_uri)
                    .expect("redirect URL")
                    .join("/favicon.ico")
                    .expect("favicon URL");
                let _ = reqwest::get(favicon).await;
                let callback = callback_url(&params.redirect_uri, "auth-code", &params.state);
                let _ = reqwest::get(callback).await;
            });
        };

        let bundle = run_pkce_flow(
            &flow_config(&format!("{}/token", server.uri())),
            &test_http(),
            &opener,
            Duration::from_secs(10),
        )
        .await
        .expect("flow");
        let SecretBundle::OAuth { access_token, .. } = bundle else {
            panic!("expected OAuth bundle");
        };
        assert_eq!(access_token, "ya29.access");
    }
}
