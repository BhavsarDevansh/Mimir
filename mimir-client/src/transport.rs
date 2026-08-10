use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// Create a new client pointing at the given base URL.
    ///
    /// The base URL should include the scheme and host/port, e.g.
    /// `http://127.0.0.1:8080`.
    ///
    /// Uses the default connect (10s) and total (120s) timeouts. A failure to
    /// build the underlying `reqwest::Client` panics, mirroring the historical
    /// behaviour; callers that prefer a fallible path should use [`Self::try_new`]
    /// (issue #165).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::try_new(
            base_url,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(120),
        )
        .expect("default reqwest client must build")
    }

    /// Create a new client with explicit timeouts, returning a [`ClientError`]
    /// instead of panicking when the HTTP client cannot be built (e.g. invalid
    /// TLS backend or missing native certificates).
    ///
    /// `connect_timeout` bounds the connection-establishment phase and `timeout`
    /// bounds an entire request. Use a large `timeout` for long-lived sessions.
    /// `base_url` is validated and normalised (trailing slashes stripped) up
    /// front so malformed input is rejected and endpoint paths never contain a
    /// double slash (PR #177 review).
    pub fn try_new(
        base_url: impl Into<String>,
        connect_timeout: std::time::Duration,
        timeout: std::time::Duration,
    ) -> Result<Self, ClientError> {
        let base_url = Self::normalize_base_url(base_url.into())?;
        let client = Self::build_client(connect_timeout, timeout)?;
        Ok(Self { base_url, client })
    }

    /// Validate and normalise `base_url`: it must parse as a hierarchical base
    /// URL (non-hierarchical schemes such as `mailto:` are rejected), and any
    /// trailing slashes are stripped so `url()` never produces `//path`.
    pub(crate) fn normalize_base_url(base_url: String) -> Result<String, ClientError> {
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|e| ClientError::Connection(format!("invalid base URL: {e}")))?;
        if parsed.cannot_be_a_base() {
            return Err(ClientError::Connection(format!(
                "invalid base URL: {base_url} is not a hierarchical base URL"
            )));
        }
        Ok(base_url.trim_end_matches('/').to_string())
    }

    /// Build the underlying `reqwest::Client`, mapping any builder failure to a
    /// [`ClientError::Connection`] so daemon/CLI startup can report the problem
    /// instead of panicking (issue #165). Extracted so the error mapping is
    /// unit-testable without a deterministic builder failure.
    pub(crate) fn build_client(
        connect_timeout: std::time::Duration,
        timeout: std::time::Duration,
    ) -> Result<reqwest::Client, ClientError> {
        reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(timeout)
            .build()
            .map_err(Self::map_build_error)
    }

    /// Map a `reqwest::Client` build failure to a [`ClientError::Connection`].
    pub(crate) fn map_build_error(e: reqwest::Error) -> ClientError {
        ClientError::Connection(format!("failed to build HTTP client: {e}"))
    }

    /// Validate the HTTP response status, returning the response on success or a
    /// [`ClientError::Server`] on failure.
    pub(crate) async fn check_response(
        resp: reqwest::Response,
    ) -> Result<reqwest::Response, ClientError> {
        let status = resp.status();
        if status.is_success() {
            Ok(resp)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    /// Validate the HTTP response status, returning `Ok(())` on success or a
    /// [`ClientError::Server`] on failure. Consolidates the bespoke status
    /// checks that previously inlined the same `Server` mapping (issue #167).
    pub(crate) async fn check_status(resp: reqwest::Response) -> Result<(), ClientError> {
        Self::check_response(resp).await?;
        Ok(())
    }

    /// Send a request builder and validate the response status, returning the
    /// raw [`reqwest::Response`] for callers that need the body as text or a
    /// byte stream. The status-check + error-mapping logic lives here so the
    /// per-method wrappers stay DRY (issue #167).
    pub(crate) async fn send_response(
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ClientError> {
        let resp = req.send().await?;
        Self::check_response(resp).await
    }

    /// Send a request builder, check the response status, and decode the JSON
    /// body. Builds on [`Self::send_response`] so the status-check + JSON-decode
    /// + error-mapping logic is centralised (issue #167).
    pub(crate) async fn send_json<T: serde::de::DeserializeOwned>(
        req: reqwest::RequestBuilder,
    ) -> Result<T, ClientError> {
        Self::send_response(req)
            .await?
            .json::<T>()
            .await
            .map_err(Into::into)
    }

    /// Issue a GET request with query parameters and decode the JSON body.
    pub(crate) async fn get_json<T: serde::de::DeserializeOwned, P: serde::Serialize + ?Sized>(
        &self,
        url: &str,
        query: &P,
    ) -> Result<T, ClientError> {
        Self::send_json(self.client.get(url).query(query)).await
    }

    /// Issue a POST request with a JSON body and decode the JSON body.
    pub(crate) async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        Self::send_json(self.client.post(url).json(body)).await
    }

    /// Build a URL by appending `path` to the configured base URL.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use mimir_api_types::{
        AuditRow, BrowseEdge, ChatMessage, ChatRequest, FactRow, OptimizationStatusResponse,
        PendingFactRow, ProfileGroup, TrashRow, Usage,
    };
    #[allow(unused_imports)]
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    #[test]
    fn try_new_builds_client_with_explicit_timeouts() {
        // Issue #165: the fallible constructor must succeed for sane timeouts.
        let client = MimirClient::try_new(
            "http://127.0.0.1:8080",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(30),
        );
        assert!(client.is_ok());
    }

    #[test]
    fn try_new_strips_trailing_slash_from_base_url() {
        // PR #177 review: a trailing slash must not produce `//path` requests.
        let client = MimirClient::try_new(
            "http://127.0.0.1:8080/",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
        // `url()` is private; exercise it indirectly via the public `url` helper
        // by checking the constructed path through a GET to a known endpoint.
        // Here we just assert the stored base via the `sessions` URL builder.
        assert_eq!(client.url("chat"), "http://127.0.0.1:8080/chat",);
    }

    #[test]
    fn try_new_rejects_malformed_base_url() {
        // PR #177 review: a non-URL base must surface as a Connection error.
        let client = MimirClient::try_new(
            "not a url",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(30),
        );
        match client {
            Err(ClientError::Connection(msg)) => assert!(
                msg.contains("invalid base URL"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[test]
    fn try_new_rejects_non_base_url() {
        // PR #177 review: a non-hierarchical URL (e.g. `mailto:`) parses
        // successfully but `cannot_be_a_base()`, so it must still be rejected
        // to avoid late failures in `url()`/`session_messages()`.
        let client = MimirClient::try_new(
            "mailto:user@example.com",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(30),
        );
        match client {
            Err(ClientError::Connection(msg)) => assert!(
                msg.contains("invalid base URL"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[test]
    fn map_build_error_produces_connection_variant() {
        // Issue #165: a reqwest client build failure must surface as
        // `ClientError::Connection` rather than a panic. reqwest accepts every
        // timeout value, so obtain a real `reqwest::Error` from a deliberately
        // invalid request URL and verify the mapping.
        let err = reqwest::Client::new()
            .get("ht!tp://invalid url")
            .build()
            .unwrap_err();
        let mapped = MimirClient::map_build_error(err);
        match mapped {
            ClientError::Connection(msg) => assert!(
                msg.contains("failed to build HTTP client"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[test]
    fn test_url_helper_builds_correct_urls() {
        let client = MimirClient::new("http://127.0.0.1:8080");
        assert_eq!(client.url("chat"), "http://127.0.0.1:8080/chat");
        assert_eq!(
            client.url("kb/categories"),
            "http://127.0.0.1:8080/kb/categories"
        );
        assert_eq!(
            client.url("chat/stream"),
            "http://127.0.0.1:8080/chat/stream"
        );
    }
}
