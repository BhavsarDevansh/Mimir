//! HTTP-level tests for the F12 rate-limit + retry/backoff primitives
//! (Phase 3 T2 / #207): a real wiremock endpoint returning 429/503 with
//! `Retry-After` is retried by `retry_with_backoff`, and a `RateLimiter`
//! with a daily quota stops issuing HTTP calls once the quota is spent.

#![deny(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use mimir_connectors::rate_limit::{
    BackoffStrategy, RateLimitConfig, RateLimitError, RateLimiter, RetryError, RetryHint,
    Retryable, retry_with_backoff,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A reqwest error wrapper that classifies HTTP statuses via the shared
/// retryable set and carries a parsed `Retry-After` header.
#[derive(Debug)]
struct HttpError {
    status: u16,
    retry_after: Option<Duration>,
}

impl Retryable for HttpError {
    fn retry_hint(&self) -> RetryHint {
        RetryHint::from_status(self.status, self.retry_after)
    }
}

/// An operation error: either an HTTP failure or a rate-limiter admission
/// rejection (quota exhausted — non-retryable).
#[derive(Debug)]
enum OpError {
    Http(HttpError),
    Quota(RateLimitError),
}

impl Retryable for OpError {
    fn retry_hint(&self) -> RetryHint {
        match self {
            OpError::Http(error) => error.retry_hint(),
            OpError::Quota(_) => RetryHint::Stop,
        }
    }
}

/// GET `path` on `server`, classifying the response into [`HttpError`].
async fn get(
    client: &reqwest::Client,
    server: &MockServer,
    path: &str,
) -> Result<String, HttpError> {
    let response = client
        .get(format!("{}{path}", server.uri()))
        .send()
        .await
        .map_err(|_| HttpError {
            status: 0,
            retry_after: None,
        })?;
    let status = response.status().as_u16();
    if status == 200 {
        return response.text().await.map_err(|_| HttpError {
            status: 0,
            retry_after: None,
        });
    }
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    Err(HttpError {
        status,
        retry_after,
    })
}

/// A fixed 1 ms backoff so tests run in milliseconds unless a server
/// `Retry-After` drives a longer wait.
fn fast_strategy() -> BackoffStrategy {
    BackoffStrategy::Fixed {
        delay: Duration::from_millis(1),
        jitter: Duration::ZERO,
    }
}

// ---------------------------------------------------------------------------
// 429 / 503 over real HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_with_backoff_retries_http_429_honouring_retry_after() {
    let server = Arc::new(MockServer::start().await);
    Mock::given(method("GET"))
        .and(path("/throttled"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/throttled"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let start = tokio::time::Instant::now();
    let body = retry_with_backoff(&fast_strategy(), 5, |_attempt| {
        let client = client.clone();
        let server = server.clone();
        Box::pin(async move { get(&client, &server, "/throttled").await })
    })
    .await
    .expect("429s must be retried");
    assert_eq!(body, "ok");
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "the server Retry-After must drive the wait, took {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn retry_with_backoff_retries_http_503_then_succeeds() {
    let server = Arc::new(MockServer::start().await);
    Mock::given(method("GET"))
        .and(path("/unavailable"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/unavailable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("recovered"))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let body = retry_with_backoff(&fast_strategy(), 5, |_attempt| {
        let client = client.clone();
        let server = server.clone();
        Box::pin(async move { get(&client, &server, "/unavailable").await })
    })
    .await
    .expect("503s must be retried");
    assert_eq!(body, "recovered");
}

// ---------------------------------------------------------------------------
// Daily quota over real HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn daily_quota_exhaustion_stops_http_calls() {
    let server = Arc::new(MockServer::start().await);
    Mock::given(method("GET"))
        .and(path("/quota"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(2)
        .mount(&server)
        .await;

    let limiter = Arc::new(
        RateLimiter::new(RateLimitConfig {
            requests_per_second: 100.0,
            burst_size: 100,
            daily_quota: Some(2),
            backoff_strategy: fast_strategy(),
        })
        .expect("limiter config"),
    );

    let client = reqwest::Client::new();
    let op = |_attempt: u32| {
        let limiter = limiter.clone();
        let client = client.clone();
        let server = server.clone();
        Box::pin(async move {
            limiter.acquire().await.map_err(OpError::Quota)?;
            get(&client, &server, "/quota").await.map_err(OpError::Http)
        })
    };

    // Two admissions fit inside the quota.
    assert_eq!(
        retry_with_backoff(&fast_strategy(), 5, op)
            .await
            .expect("first call"),
        "ok"
    );
    assert_eq!(
        retry_with_backoff(&fast_strategy(), 5, op)
            .await
            .expect("second call"),
        "ok"
    );

    // The third admission is rejected before any HTTP call is made, and the
    // rejection is non-retryable (the wiremock `expect(2)` verifies no third
    // request reached the server).
    let err = retry_with_backoff(&fast_strategy(), 5, op)
        .await
        .expect_err("quota must be exhausted");
    assert!(
        matches!(
            err,
            RetryError::Terminal(OpError::Quota(RateLimitError::QuotaExhausted { .. }))
        ),
        "expected a terminal quota-exhausted error, got {err:?}"
    );
}
