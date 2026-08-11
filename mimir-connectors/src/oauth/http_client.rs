//! `oauth2`-crate HTTP client adapter over the workspace reqwest 0.13 client.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use oauth2::{AsyncHttpClient, HttpClientError, HttpRequest, HttpResponse};

use crate::connector::ConnectorError;

/// [`oauth2::AsyncHttpClient`] implementation backed by the workspace's
/// reqwest 0.13 client.
///
/// `oauth2` 5.0.0's own reqwest implementation is feature-gated and pins
/// `reqwest 0.12`, which would duplicate the workspace's reqwest 0.13
/// HTTP/TLS stack. The crate's `HttpRequest`/`HttpResponse` are plain
/// `http` 1.x types (shared with reqwest 0.13), so this adapter implements
/// `AsyncHttpClient` directly over the workspace client — the same pattern as
/// `oauth2`'s own `reqwest_client.rs`, with no reqwest 0.12 in the tree.
///
/// # Redirect policy
///
/// The client is built with [`reqwest::redirect::Policy::none`]: OAuth token
/// requests carry credentials, and following a redirect would let a
/// compromised or malicious token endpoint bounce the refresh grant (or, in
/// A4 / #205, the authorization-code exchange) to an attacker-controlled
/// host. This matches the `oauth2` crate's own SSRF guidance.
#[derive(Clone, Debug)]
pub struct OAuthHttpClient(reqwest::Client);

impl OAuthHttpClient {
    /// Build a hardened OAuth HTTP client: 30 s timeout, redirects disabled.
    pub fn new() -> Result<Self, ConnectorError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ConnectorError::Config(format!("OAuth HTTP client build failed: {e}")))?;
        Ok(Self(client))
    }

    /// Wrap an existing client. Used by tests to point the adapter at a mock
    /// server; the caller is responsible for the redirect policy.
    pub fn from_client(client: reqwest::Client) -> Self {
        Self(client)
    }
}

impl<'c> AsyncHttpClient<'c> for OAuthHttpClient {
    type Error = HttpClientError<reqwest::Error>;
    type Future =
        Pin<Box<dyn Future<Output = Result<HttpResponse, Self::Error>> + Send + Sync + 'c>>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let response = self
                .0
                .execute(request.try_into().map_err(Box::new)?)
                .await
                .map_err(Box::new)?;
            let mut builder = http::Response::builder().status(response.status());
            for (name, value) in response.headers().iter() {
                builder = builder.header(name, value);
            }
            builder
                .body(response.bytes().await.map_err(Box::new)?.to_vec())
                .map_err(HttpClientError::Http)
        })
    }
}
