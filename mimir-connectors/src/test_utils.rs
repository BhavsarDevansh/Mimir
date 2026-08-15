//! Shared OAuth test doubles (Phase 3 / issues #290, #298).
//!
//! The interactive PKCE flow (A4 / #205) accepts an `open_browser` callback:
//! production code opens the user's browser, but tests inject a fake browser
//! that parses the authorize URL for the redirect URI + CSRF state and then
//! GETs the loopback callback with a canned code — exactly what a real
//! browser does. The flow's unit tests (`oauth::pkce`), the CLI connector
//! tests (`mimir/src/connector/tests.rs`), and the flow's inline variant
//! openers all used to re-implement this parsing locally, and both suites
//! used to inline the wiremock token-endpoint mock for the code exchange,
//! so the shared helpers live here once.
//!
//! Test-only infrastructure, gated by the `test-utils` feature (off by
//! default). The crate's own unit tests compile it via `cfg(test)`, and
//! downstream crates opt in with the feature.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Query parameters the fake browser needs from the authorize URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeUrlParams {
    /// The loopback `redirect_uri` the provider will bounce the browser to.
    pub redirect_uri: String,
    /// The CSRF `state` the flow generated; the callback must echo it.
    pub state: String,
}

/// Parse the `redirect_uri` and CSRF `state` out of an authorize URL.
///
/// The drift-prone part of every fake-browser opener (a new query parameter
/// must not be learned by one copy and forgotten by another), so the parsing
/// lives here once and all openers build on it.
pub fn parse_authorize_url(url: &str) -> Result<AuthorizeUrlParams, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("authorize URL is invalid: {e}"))?;
    let mut redirect_uri = None;
    let mut state = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "redirect_uri" => redirect_uri = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            _ => {}
        }
    }
    Ok(AuthorizeUrlParams {
        redirect_uri: redirect_uri.ok_or("authorize URL is missing redirect_uri")?,
        state: state.ok_or("authorize URL is missing state")?,
    })
}

/// Build the loopback callback URL a browser hits after the provider
/// redirects: the `redirect_uri` plus the code and echoed CSRF state.
///
/// The code and state are appended as percent-encoded query pairs (a value
/// containing `+`, `#`, `%`, or `&` must not be mangled or re-parsed as a
/// different parameter), and an existing query on the redirect URI is
/// preserved with the correct `&` separator.
pub fn callback_url(redirect_uri: &str, code: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(redirect_uri).expect("redirect_uri is a valid URL");
    url.query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", state);
    url.to_string()
}

/// An opener that drives the loopback callback itself: parses the authorize
/// URL for the redirect URI + CSRF state, then GETs the callback with a
/// canned code — exactly what a real browser does.
pub fn self_callback_opener(code: &'static str) -> impl Fn(&str) + Send + Sync {
    move |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            let params = parse_authorize_url(&url).expect("authorize URL");
            let callback = callback_url(&params.redirect_uri, code, &params.state);
            let _ = reqwest::get(callback).await;
        });
    }
}

/// Mount a wiremock token endpoint that answers the PKCE code exchange.
///
/// The response is the canonical OAuth token shape (access token, token
/// type, refresh token, expiry) so the shape cannot drift between the PKCE
/// flow's unit tests and the CLI connector tests. `expected_calls` is the
/// number of POSTs the test expects; wiremock fails the test if the actual
/// count differs.
pub async fn mount_token_endpoint(server: &MockServer, expected_calls: u64) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "ya29.access",
            "token_type": "Bearer",
            "refresh_token": "rt",
            "expires_in": 3600,
        })))
        .expect(expected_calls)
        .mount(server)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn parse_authorize_url_extracts_redirect_uri_and_state() {
        let params = parse_authorize_url(
            "https://oauth.example.com/authorize?state=csrf-token&redirect_uri=http%3A%2F%2F127.0.0.1%3A43123%2Fcallback",
        )
        .expect("parse");
        assert_eq!(params.redirect_uri, "http://127.0.0.1:43123/callback");
        assert_eq!(params.state, "csrf-token");
    }

    #[test]
    fn parse_authorize_url_rejects_missing_state_or_redirect_uri() {
        let err = parse_authorize_url("https://oauth.example.com/authorize?state=x")
            .expect_err("missing redirect_uri must fail");
        assert!(err.contains("redirect_uri"), "got: {err}");

        let err = parse_authorize_url("https://oauth.example.com/authorize?redirect_uri=x")
            .expect_err("missing state must fail");
        assert!(err.contains("state"), "got: {err}");
    }

    #[test]
    fn callback_url_appends_code_and_state() {
        assert_eq!(
            callback_url("http://127.0.0.1:43123/callback", "auth-code", "csrf-token"),
            "http://127.0.0.1:43123/callback?code=auth-code&state=csrf-token"
        );
    }

    #[test]
    fn callback_url_percent_encodes_reserved_characters_and_preserves_query() {
        assert_eq!(
            callback_url(
                "http://127.0.0.1:43123/callback?existing=1",
                "a+b/c",
                "s#t&t"
            ),
            "http://127.0.0.1:43123/callback?existing=1&code=a%2Bb%2Fc&state=s%23t%26t"
        );
    }

    #[tokio::test]
    async fn self_callback_opener_drives_the_loopback_callback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let redirect_uri = format!(
            "http://{}/callback",
            listener.local_addr().expect("local addr")
        );
        let authorize_url = format!(
            "https://oauth.example.com/authorize?state=csrf-token&redirect_uri={redirect_uri}"
        );

        self_callback_opener("auth-code")(&authorize_url);

        // The fake browser must GET the loopback callback with the code and
        // the CSRF state echoed — the request the real flow parses. Every
        // wait is bounded so a broken opener fails the test with a clear
        // message instead of hanging it.
        let (mut socket, _peer) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("timed out waiting for the callback connection")
            .expect("callback connection");
        let mut request = Vec::new();
        let mut chunk = [0u8; 128];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut chunk))
                .await
                .expect("timed out reading the callback request")
                .expect("read request");
            if n == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..n]);
            if request.contains(&b'\n') {
                break;
            }
        }
        let request = String::from_utf8(request).expect("utf-8 request");
        assert!(
            request.starts_with("GET /callback?code=auth-code&state=csrf-token "),
            "got: {request}"
        );

        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .expect("respond");
    }

    #[tokio::test]
    async fn mount_token_endpoint_answers_the_code_exchange() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, 1).await;

        let response = reqwest::Client::new()
            .post(format!("{}/token", server.uri()))
            .send()
            .await
            .expect("token request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("token body");
        assert_eq!(body["access_token"], "ya29.access");
        assert_eq!(body["token_type"], "Bearer");
        assert_eq!(body["refresh_token"], "rt");
        assert_eq!(body["expires_in"], 3600);
    }
}
