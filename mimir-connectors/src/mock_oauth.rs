//! In-process mock OAuth 2.0 authorization server for tests (Phase 3 T2 / #207).
//!
//! Serves the two endpoints the interactive PKCE flow (A4 / #205) needs
//! without a real provider, on loopback listeners sharing one state object:
//!
//! - **`GET /authorize` over HTTPS** — the browser-facing endpoint. The
//!   flow's `auth_uri` gate requires HTTPS (the authorization endpoint
//!   carries the user's credentials; RFC 6749 §3.1), so the mock serves
//!   TLS with a self-signed certificate generated at test runtime. It
//!   records the request, issues a one-time authorization code, and answers
//!   `302 Found` with the code and the client's CSRF `state` echoed back to
//!   the `redirect_uri`.
//! - **`POST /token` over HTTP** — the code-exchange endpoint. Loopback HTTP
//!   is Mimir's local trust boundary (the shared token-endpoint gate permits
//!   it), and the production [`OAuthHttpClient`](crate::oauth::OAuthHttpClient)
//!   cannot trust a test certificate, so this listener stays plain HTTP. It
//!   validates the PKCE S256 `code_verifier` against the challenge captured
//!   at authorize time, enforces one-time use of the code, and issues an
//!   OAuth token bundle.
//!
//! The server is test-only infrastructure, gated by the `test-mock-oauth`
//! feature (off by default). It is used by this crate's PKCE E2E tests and by
//! the `mimir` binary's daemon-level connector tests.
//!
//! # Security notes
//!
//! The self-signed certificate is generated per test run and never leaves the
//! process; clients that talk to the authorize endpoint (the fake browser)
//! must skip certificate verification, which is acceptable because the
//! endpoint is bound to loopback and carries no real credentials.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use rand::Rng;
use sha2::Digest;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, watch};
use tokio_rustls::TlsAcceptor;

/// Upper bound on a request (request line + headers + form body). A browser
/// authorize GET is a few hundred bytes; a token POST a few hundred more.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// A single authorization-code issuance, keyed by the one-time code.
struct IssuedCode {
    code_challenge: String,
    redirect_uri: String,
}

/// A recorded `GET /authorize` request, for test assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub scope: Option<String>,
}

/// A recorded `POST /token` request, for test assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub code_verifier: String,
}

/// Shared state between the two listeners.
#[derive(Default)]
struct MockOAuthState {
    codes: Mutex<HashMap<String, IssuedCode>>,
    authorize_requests: Mutex<Vec<AuthorizeRequest>>,
    token_requests: Mutex<Vec<TokenRequest>>,
}

/// In-process mock OAuth 2.0 authorization server (Phase 3 T2 / #207).
///
/// [`MockOAuthServer::start`] binds two loopback listeners (HTTPS
/// `/authorize` + HTTP `/token`) on ephemeral ports and serves them on a
/// dedicated thread until the server is dropped. The returned URLs are the
/// values to feed [`PkceFlowConfig`](crate::oauth::PkceFlowConfig).
pub struct MockOAuthServer {
    authorize_url: String,
    token_url: String,
    state: Arc<MockOAuthState>,
    shutdown: watch::Sender<bool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl MockOAuthServer {
    /// Bind and start the mock server. Panics if a listener cannot bind or
    /// the self-signed certificate cannot be generated (test-only).
    pub fn start() -> Self {
        // rustls 0.23 needs a crypto provider installed; reqwest's rustls
        // feature installs the same aws-lc-rs provider, so a second install
        // (when this runs before any reqwest client is built) is a no-op.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
            .expect("generate self-signed certificate");
        let tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![certified.cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        certified.signing_key.serialize_der(),
                    ),
                ),
            )
            .expect("build TLS server config");
        let acceptor = TlsAcceptor::from(Arc::new(tls_config));

        let state = Arc::new(MockOAuthState::default());
        let (shutdown, rx) = watch::channel(false);

        // Bind inside each serving runtime (tokio rejects sockets created on a
        // foreign thread) and hand the bound address back so `start()` can
        // return valid URLs immediately.
        let (tls_addr_tx, tls_addr_rx) = std::sync::mpsc::channel();
        let state_tls = Arc::clone(&state);
        let rx_tls = rx.clone();
        let tls_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("TLS serving runtime");
            runtime.block_on(async move {
                let listener = TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .expect("bind TLS listener");
                let addr = listener.local_addr().expect("TLS listener address");
                tls_addr_tx.send(addr).expect("send TLS listener address");
                serve_authorize(listener, acceptor, state_tls, rx_tls).await;
            });
        });

        let (http_addr_tx, http_addr_rx) = std::sync::mpsc::channel();
        let state_http = Arc::clone(&state);
        let rx_http = rx.clone();
        let http_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("HTTP serving runtime");
            runtime.block_on(async move {
                let listener = TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .expect("bind HTTP listener");
                let addr = listener.local_addr().expect("HTTP listener address");
                http_addr_tx.send(addr).expect("send HTTP listener address");
                serve_token(listener, state_http, rx_http).await;
            });
        });
        let tls_addr = tls_addr_rx.recv().expect("TLS listener address");
        let http_addr = http_addr_rx.recv().expect("HTTP listener address");

        Self {
            authorize_url: format!("https://{tls_addr}/authorize"),
            token_url: format!("http://{http_addr}/token"),
            state,
            shutdown,
            threads: vec![tls_thread, http_thread],
        }
    }

    /// The HTTPS authorize endpoint to use as `auth_uri`.
    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    /// The HTTP token endpoint to use as `token_endpoint`.
    pub fn token_url(&self) -> &str {
        &self.token_url
    }

    /// Authorize requests received so far, in order. Only requests that pass
    /// validation are recorded — a rejected authorize request never appears
    /// here (tests assert on this).
    pub async fn authorize_requests(&self) -> Vec<AuthorizeRequest> {
        self.state.authorize_requests.lock().await.clone()
    }

    /// Token requests received so far, in order. Unlike authorize requests,
    /// token requests are recorded *before* validation, so rejected exchanges
    /// (wrong verifier, replayed code, unknown grant type) do appear here.
    pub async fn token_requests(&self) -> Vec<TokenRequest> {
        self.state.token_requests.lock().await.clone()
    }

    /// The PKCE S256 challenge for `verifier` — the value the authorize
    /// endpoint records and the token endpoint checks. Exposed so tests can
    /// craft valid authorize/token request pairs directly.
    pub fn s256_challenge(verifier: &str) -> String {
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }
}

impl Drop for MockOAuthServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Serve the HTTPS authorize endpoint until shutdown.
async fn serve_authorize(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    state: Arc<MockOAuthState>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(_) => continue,
                };
                let acceptor = acceptor.clone();
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        let _ = handle_authorize(&mut tls, &state).await;
                    }
                });
            }
        }
    }
}

/// Serve the HTTP token endpoint until shutdown.
async fn serve_token(
    listener: TcpListener,
    state: Arc<MockOAuthState>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => {
                let (mut stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(_) => continue,
                };
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _ = handle_token(&mut stream, &state).await;
                });
            }
        }
    }
}

/// A parsed HTTP request: method, request target, and query/form parameters.
struct HttpRequest {
    method: String,
    target: String,
    params: HashMap<String, String>,
}

/// Read one bounded HTTP/1.1 request (headers + optional form body).
async fn read_request<S>(stream: &mut S) -> std::io::Result<Option<HttpRequest>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() >= MAX_REQUEST_BYTES {
            return Ok(None);
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() {
        return Ok(None);
    }
    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    // The body may already be in the buffer after the header terminator
    // (headers + body can arrive in one read); only read more when needed.
    let mut body = buf[header_end..].to_vec();
    if content_length > 0 {
        if content_length > MAX_REQUEST_BYTES {
            return Ok(None);
        }
        while body.len() < content_length {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(None);
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(content_length);
    }
    let params = if method == "POST" {
        url::form_urlencoded::parse(&body).into_owned().collect()
    } else {
        let (_, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect()
    };
    Ok(Some(HttpRequest {
        method,
        target,
        params,
    }))
}

/// Handle `GET /authorize`: record the request, issue a one-time code, and
/// redirect to the client's `redirect_uri` with the CSRF `state` echoed.
async fn handle_authorize<S>(stream: &mut S, state: &MockOAuthState) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(request) = read_request(stream).await? else {
        return Ok(());
    };
    if request.method != "GET" {
        return respond(stream, 405, "Method Not Allowed", &[], &[]).await;
    }
    let (path, _) = request
        .target
        .split_once('?')
        .unwrap_or((&request.target, ""));
    if path != "/authorize" {
        return respond(stream, 404, "Not Found", &[], &[]).await;
    }
    let get = |key: &str| request.params.get(key).cloned();
    let Some(client_id) = get("client_id") else {
        return respond(stream, 400, "Bad Request", &[], &[]).await;
    };
    let Some(redirect_uri) = get("redirect_uri") else {
        return respond(stream, 400, "Bad Request", &[], &[]).await;
    };
    if redirect_uri.contains(['\r', '\n']) {
        // The redirect URI is echoed into the `Location` header; reject CR/LF
        // so a hostile caller cannot inject response headers (test-only
        // server, but the guard is free).
        return respond(stream, 400, "Bad Request", &[], &[]).await;
    }
    let Some(state_param) = get("state") else {
        return respond(stream, 400, "Bad Request", &[], &[]).await;
    };
    if state_param.contains(['\r', '\n']) {
        // `state` is echoed into the `Location` header alongside the redirect
        // URI; reject CR/LF for the same header-injection reason.
        return respond(stream, 400, "Bad Request", &[], &[]).await;
    }
    let code_challenge = get("code_challenge").unwrap_or_default();
    let code_challenge_method = get("code_challenge_method").unwrap_or_default();
    if code_challenge_method != "S256" {
        // The token endpoint only validates S256 challenges; reject anything
        // else at authorize time so a code that can never be exchanged is
        // never issued.
        return respond(stream, 400, "Bad Request", &[], &[]).await;
    }
    let scope = get("scope");

    state
        .authorize_requests
        .lock()
        .await
        .push(AuthorizeRequest {
            client_id: client_id.clone(),
            redirect_uri: redirect_uri.clone(),
            state: state_param.clone(),
            code_challenge: code_challenge.clone(),
            code_challenge_method: code_challenge_method.clone(),
            scope,
        });

    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    state.codes.lock().await.insert(
        code.clone(),
        IssuedCode {
            code_challenge,
            redirect_uri: redirect_uri.clone(),
        },
    );

    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", &code)
        .append_pair("state", &state_param)
        .finish();
    let location = format!("{redirect_uri}?{query}");
    respond(
        stream,
        302,
        "Found",
        &[("Location", location.as_str())],
        &[],
    )
    .await
}

/// Handle `POST /token`: validate the one-time code and PKCE verifier, then
/// issue an OAuth token bundle.
async fn handle_token<S>(stream: &mut S, state: &MockOAuthState) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(request) = read_request(stream).await? else {
        return Ok(());
    };
    if request.method != "POST" {
        return respond(stream, 405, "Method Not Allowed", &[], &[]).await;
    }
    if request.target != "/token" {
        return respond(stream, 404, "Not Found", &[], &[]).await;
    }
    let get = |key: &str| request.params.get(key).cloned();
    let grant_type = get("grant_type").unwrap_or_default();
    let code = get("code").unwrap_or_default();
    let redirect_uri = get("redirect_uri").unwrap_or_default();
    let client_id = get("client_id").unwrap_or_default();
    let code_verifier = get("code_verifier").unwrap_or_default();

    state.token_requests.lock().await.push(TokenRequest {
        grant_type: grant_type.clone(),
        code: code.clone(),
        redirect_uri: redirect_uri.clone(),
        client_id: client_id.clone(),
        code_verifier: code_verifier.clone(),
    });

    if grant_type != "authorization_code" {
        return token_error(stream, "unsupported_grant_type").await;
    }
    let mut codes = state.codes.lock().await;
    let Some(issued) = codes.remove(&code) else {
        return token_error(stream, "invalid_grant").await;
    };
    if issued.redirect_uri != redirect_uri {
        return token_error(stream, "invalid_grant").await;
    }
    if MockOAuthServer::s256_challenge(&code_verifier) != issued.code_challenge {
        return token_error(stream, "invalid_grant").await;
    }

    let body = serde_json::to_vec(&serde_json::json!({
        "access_token": "mock-access-token",
        "token_type": "Bearer",
        "refresh_token": "mock-refresh-token",
        "expires_in": 3600,
    }))
    .expect("token JSON");
    respond(
        stream,
        200,
        "OK",
        &[("Content-Type", "application/json")],
        &body,
    )
    .await
}

/// Respond with an RFC 6749 §5.2 error payload.
async fn token_error<S>(stream: &mut S, error: &str) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(&serde_json::json!({ "error": error })).expect("error JSON");
    respond(
        stream,
        400,
        "Bad Request",
        &[("Content-Type", "application/json")],
        &body,
    )
    .await
}

/// Write a minimal HTTP/1.1 response.
async fn respond<S>(
    stream: &mut S,
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}
