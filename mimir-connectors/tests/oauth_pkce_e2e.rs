//! E2E tests for the interactive PKCE flow (A4 / #205) against the in-process
//! mock OAuth server (Phase 3 T2 / #207): the full authorize → redirect →
//! loopback callback → code exchange round trip without a real provider.
//!
//! Gated on the `test-mock-oauth` feature (off by default). `cargo test
//! --workspace` enables it through the `mimir` binary's dev-dependencies, so
//! the suite runs in the standard workspace test run; a standalone
//! `cargo test -p mimir-connectors` needs `--features test-mock-oauth`.

#![cfg(feature = "test-mock-oauth")]
#![deny(unsafe_code)]

use std::collections::HashMap;
use std::time::Duration;

use mimir_connectors::SecretBundle;
use mimir_connectors::mock_oauth::MockOAuthServer;
use mimir_connectors::oauth::{OAuthHttpClient, PkceFlowConfig, run_pkce_flow};

/// The OAuth client configuration the flow needs, pointed at the mock server.
fn flow_config(server: &MockOAuthServer) -> PkceFlowConfig {
    PkceFlowConfig {
        auth_uri: server.authorize_url().to_string(),
        token_endpoint: server.token_url().to_string(),
        client_id: "mimir-test-client".to_string(),
        client_secret: None,
        scopes: Some(vec!["read".to_string(), "write".to_string()]),
    }
}

/// A fake browser: GETs the authorize URL (accepting the mock's self-signed
/// certificate) and follows the redirect into the loopback callback — exactly
/// what a real browser does.
fn browser_opener() -> impl Fn(&str) + Send + Sync {
    |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .expect("test HTTP client");
            let _ = client.get(url).send().await;
        });
    }
}

/// Build an authorize URL for a direct (non-flow) request against the mock.
fn authorize_url(
    server: &MockOAuthServer,
    redirect_uri: &str,
    state: &str,
    verifier: &str,
) -> String {
    let challenge = MockOAuthServer::s256_challenge(verifier);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", "direct-client")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{}?{query}", server.authorize_url())
}

/// Extract the one-time code from a mock authorize `302` redirect.
fn code_from_redirect(location: &str) -> String {
    let parsed = reqwest::Url::parse(location).expect("redirect URL");
    parsed
        .query_pairs()
        .find(|(k, _)| k == "code")
        .expect("code param")
        .1
        .into_owned()
}

/// Exchange `code` at the mock token endpoint with `verifier`.
async fn exchange(
    client: &reqwest::Client,
    server: &MockOAuthServer,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> reqwest::Response {
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("client_id", "direct-client")
        .append_pair("code_verifier", verifier)
        .finish();
    client
        .post(server.token_url())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .expect("token request")
}

/// A reqwest client that accepts the mock's self-signed certificate and does
/// not follow redirects (the direct tests assert on the `302` itself).
fn tls_skipping_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("test HTTP client")
}

// ---------------------------------------------------------------------------
// Full flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_pkce_flow_round_trips_against_mock_server() {
    let server = MockOAuthServer::start();
    let bundle = run_pkce_flow(
        &flow_config(&server),
        &OAuthHttpClient::new().expect("OAuth HTTP client"),
        &browser_opener(),
        Duration::from_secs(10),
    )
    .await
    .expect("the flow must complete against the mock server");

    let SecretBundle::OAuth {
        access_token,
        refresh_token,
        expires_at,
    } = bundle
    else {
        panic!("expected an OAuth bundle");
    };
    assert_eq!(access_token, "mock-access-token");
    assert_eq!(refresh_token.as_deref(), Some("mock-refresh-token"));
    assert!(expires_at.is_some(), "the bundle must carry an expiry");

    // The authorize endpoint saw exactly one request with the expected shape.
    let authorizes = server.authorize_requests().await;
    assert_eq!(authorizes.len(), 1, "exactly one authorize request");
    assert_eq!(authorizes[0].client_id, "mimir-test-client");
    assert!(
        authorizes[0].redirect_uri.starts_with("http://127.0.0.1:")
            && authorizes[0].redirect_uri.ends_with("/callback"),
        "redirect_uri must be the loopback callback, got {}",
        authorizes[0].redirect_uri
    );
    assert!(
        !authorizes[0].state.is_empty(),
        "CSRF state must be present"
    );
    assert!(
        !authorizes[0].code_challenge.is_empty(),
        "PKCE S256 challenge must be present"
    );
    assert_eq!(authorizes[0].code_challenge_method, "S256");
    assert_eq!(
        authorizes[0].scope.as_deref(),
        Some("read write"),
        "scopes are joined into the authorize URL"
    );

    // The token endpoint saw exactly one exchange, and the mock validated the
    // S256 verifier against the challenge (the flow only succeeds if the
    // verifier matches).
    let tokens = server.token_requests().await;
    assert_eq!(tokens.len(), 1, "exactly one token exchange");
    assert_eq!(tokens[0].grant_type, "authorization_code");
    assert_eq!(tokens[0].client_id, "mimir-test-client");
    assert!(
        !tokens[0].code_verifier.is_empty(),
        "the exchange must carry a PKCE verifier"
    );
}

// ---------------------------------------------------------------------------
// Mock-server correctness (direct requests)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_authorize_redirects_with_state_echo_and_one_time_code() {
    let server = MockOAuthServer::start();
    let client = tls_skipping_client();
    let redirect_uri = "http://127.0.0.1:9999/cb";

    let response = client
        .get(authorize_url(
            &server,
            redirect_uri,
            "echo-me",
            "test-verifier",
        ))
        .send()
        .await
        .expect("authorize request");
    assert_eq!(response.status(), 302);

    let location = response
        .headers()
        .get("location")
        .expect("Location header")
        .to_str()
        .expect("Location is ASCII")
        .to_string();
    let parsed = reqwest::Url::parse(&location).expect("redirect URL");
    assert_eq!(parsed.scheme(), "http");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(9999));
    assert_eq!(parsed.path(), "/cb");
    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    assert_eq!(
        params.get("state").map(String::as_str),
        Some("echo-me"),
        "the CSRF state must be echoed back"
    );
    let code = params.get("code").expect("one-time code").clone();
    assert!(!code.is_empty());

    // The code exchanges once...
    let first = exchange(&client, &server, &code, redirect_uri, "test-verifier").await;
    assert_eq!(first.status(), 200, "first exchange succeeds");

    // ...and a replay of the same code is rejected.
    let replay = exchange(&client, &server, &code, redirect_uri, "test-verifier").await;
    assert_eq!(replay.status(), 400);
    let body: serde_json::Value = replay.json().await.expect("error JSON");
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn token_endpoint_rejects_wrong_pkce_verifier() {
    let server = MockOAuthServer::start();
    let client = tls_skipping_client();
    let redirect_uri = "http://127.0.0.1:9999/cb";

    // Authorize with the real challenge, then exchange with a wrong verifier.
    let response = client
        .get(authorize_url(&server, redirect_uri, "s", "real-verifier"))
        .send()
        .await
        .expect("authorize request");
    let location = response
        .headers()
        .get("location")
        .expect("Location header")
        .to_str()
        .expect("Location is ASCII")
        .to_string();
    let code = code_from_redirect(&location);

    let response = exchange(&client, &server, &code, redirect_uri, "wrong-verifier").await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("error JSON");
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn token_endpoint_rejects_unknown_grant_type() {
    let server = MockOAuthServer::start();
    let client = tls_skipping_client();
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", "stale")
        .finish();
    let response = client
        .post(server.token_url())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .expect("token request");
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("error JSON");
    assert_eq!(body["error"], "unsupported_grant_type");
}

#[tokio::test]
async fn authorize_rejects_non_s256_challenge_method() {
    let server = MockOAuthServer::start();
    let client = tls_skipping_client();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", "direct-client")
        .append_pair("redirect_uri", "http://127.0.0.1:9999/cb")
        .append_pair("state", "s")
        .append_pair("code_challenge", "not-a-challenge")
        .append_pair("code_challenge_method", "plain")
        .finish();
    let response = client
        .get(format!("{}?{query}", server.authorize_url()))
        .send()
        .await
        .expect("authorize request");
    assert_eq!(
        response.status(),
        400,
        "the mock only supports S256 challenges"
    );
    assert!(
        server.authorize_requests().await.is_empty(),
        "a rejected authorize request must not be recorded"
    );
}

#[tokio::test]
async fn authorize_rejects_crlf_in_redirect_uri() {
    let server = MockOAuthServer::start();
    let client = tls_skipping_client();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", "direct-client")
        .append_pair("redirect_uri", "http://127.0.0.1:9999/cb\r\nX-Evil: 1")
        .append_pair("state", "s")
        .append_pair("code_challenge", "challenge")
        .append_pair("code_challenge_method", "S256")
        .finish();
    let response = client
        .get(format!("{}?{query}", server.authorize_url()))
        .send()
        .await
        .expect("authorize request");
    assert_eq!(
        response.status(),
        400,
        "CR/LF in the redirect URI must be rejected"
    );
    assert!(
        response.headers().get("x-evil").is_none(),
        "no header may be injected through the redirect URI"
    );
    assert!(
        server.authorize_requests().await.is_empty(),
        "a rejected authorize request must not be recorded"
    );
}

#[tokio::test]
async fn authorize_rejects_crlf_in_state() {
    let server = MockOAuthServer::start();
    let client = tls_skipping_client();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", "direct-client")
        .append_pair("redirect_uri", "http://127.0.0.1:9999/cb")
        .append_pair("state", "s\r\nX-Evil: 1")
        .append_pair("code_challenge", "challenge")
        .append_pair("code_challenge_method", "S256")
        .finish();
    let response = client
        .get(format!("{}?{query}", server.authorize_url()))
        .send()
        .await
        .expect("authorize request");
    assert_eq!(
        response.status(),
        400,
        "CR/LF in the state must be rejected"
    );
    assert!(
        response.headers().get("x-evil").is_none(),
        "no header may be injected through the state"
    );
    assert!(
        server.authorize_requests().await.is_empty(),
        "a rejected authorize request must not be recorded"
    );
}
