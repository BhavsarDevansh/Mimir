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
///
/// # Response body bound
///
/// Token responses are a few hundred bytes; the body read is capped at
/// `MAX_RESPONSE_BYTES` so a compromised or misconfigured endpoint cannot
/// force a large allocation on the refresh path of a long-running daemon.
#[derive(Clone, Debug)]
pub struct OAuthHttpClient(reqwest::Client);

/// Upper bound on a token-endpoint response body. A token response is a few
/// hundred bytes; anything larger is rejected rather than buffered whole.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

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
    #[cfg(test)]
    pub(crate) fn from_client(client: reqwest::Client) -> Self {
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
            let mut body = Vec::new();
            let mut response = response;
            while let Some(chunk) = response.chunk().await.map_err(Box::new)? {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(HttpClientError::Other(format!(
                        "token response body exceeds {MAX_RESPONSE_BYTES} bytes"
                    )));
                }
                body.extend_from_slice(&chunk);
            }
            builder.body(body).map_err(HttpClientError::Http)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn call_rejects_oversized_response_body() {
        // A hostile or misconfigured token endpoint must not be able to force
        // a large allocation: the adapter caps the body read and rejects
        // anything over the bound.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 70 * 1024]))
            .mount(&server)
            .await;

        let client = OAuthHttpClient::from_client(reqwest::Client::new());
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("{}/token", server.uri()))
            .body(Vec::new())
            .expect("request");
        let err = client
            .call(request)
            .await
            .expect_err("oversized body must be rejected");
        assert!(
            matches!(err, HttpClientError::Other(_)),
            "expected Other error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn call_accepts_small_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 1024]))
            .mount(&server)
            .await;

        let client = OAuthHttpClient::from_client(reqwest::Client::new());
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("{}/token", server.uri()))
            .body(Vec::new())
            .expect("request");
        let response = client.call(request).await.expect("small body must pass");
        assert_eq!(response.body().len(), 1024);
    }
}
