//! Integration tests for the shared rate-limit + retry/backoff primitives
//! (Phase 3 F12 / issue #189).
//!
//! These exercise the *public* surface of [`mimir_connectors::rate_limit`]:
//! token-bucket throttling, daily-quota exhaustion, 429/503 retry with
//! backoff, `Retry-After` honouring, and config serde/presets. Pure-logic
//! helpers (backoff progression, status classification, quota-window reset
//! with an injected clock) are unit-tested inline in the module itself.

#![deny(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use chrono::Utc;

use mimir_connectors::rate_limit::{
    BackoffStrategy, RateLimitConfig, RateLimitError, RateLimiter, RetryError, RetryHint,
    Retryable, is_retryable_status, retry_with_backoff,
};

// ---------------------------------------------------------------------------
// Test error type implementing Retryable
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestErr {
    Transient,
    Status(u16),
    Terminal,
}

impl Retryable for TestErr {
    fn retry_hint(&self) -> RetryHint {
        match self {
            TestErr::Transient => RetryHint::Retry { retry_after: None },
            TestErr::Status(code) => RetryHint::from_status(*code, None),
            TestErr::Terminal => RetryHint::Stop,
        }
    }
}

/// Builds an operation that fails `failures` times with `err` then returns
/// `Ok(value)`. Records the number of invocations in `calls`.
fn flaky_op<T: Clone + Send + 'static>(
    failures: u32,
    err: TestErr,
    value: T,
    calls: Arc<AtomicU32>,
) -> impl FnMut(u32) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, TestErr>> + Send>>
{
    move |_attempt: u32| {
        let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
        let err = err.clone();
        let value = value.clone();
        Box::pin(async move { if n <= failures { Err(err) } else { Ok(value) } })
    }
}

// ---------------------------------------------------------------------------
// Config + serde
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_config_round_trips_with_all_backoff_variants() {
    let cfg = RateLimitConfig {
        requests_per_second: 1.5,
        burst_size: 4,
        daily_quota: Some(10_000),
        backoff_strategy: BackoffStrategy::Exponential {
            base: Duration::from_millis(500),
            max: Duration::from_secs(30),
            jitter: Duration::from_millis(250),
        },
    };
    let json = serde_json::to_value(&cfg).unwrap();
    assert_eq!(json["requests_per_second"], 1.5);
    assert_eq!(json["burst_size"], 4);
    assert_eq!(json["daily_quota"], 10_000);
    let back: RateLimitConfig = serde_json::from_value(json).unwrap();
    assert_eq!(cfg, back);

    let linear = RateLimitConfig {
        requests_per_second: 0.5,
        burst_size: 1,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::Linear {
            base: Duration::from_millis(100),
            step: Duration::from_millis(100),
            max: Duration::from_secs(5),
            jitter: Duration::from_millis(50),
        },
    };
    let j = serde_json::to_value(&linear).unwrap();
    assert_eq!(j["backoff_strategy"]["kind"], "linear");
    let back: RateLimitConfig = serde_json::from_value(j).unwrap();
    assert_eq!(linear, back);

    let fixed = RateLimitConfig {
        requests_per_second: 2.0,
        burst_size: 2,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::Fixed {
            delay: Duration::from_millis(750),
            jitter: Duration::ZERO,
        },
    };
    let j = serde_json::to_value(&fixed).unwrap();
    assert_eq!(j["backoff_strategy"]["kind"], "fixed");
    assert_eq!(serde_json::from_value::<RateLimitConfig>(j).unwrap(), fixed);
}

#[test]
fn backoff_strategy_default_is_exponential() {
    let s = BackoffStrategy::default();
    assert!(matches!(s, BackoffStrategy::Exponential { .. }));
}

#[test]
fn nominatim_preset_respects_usage_policy() {
    // Nominatim's usage policy: ≤ 1 request/second, identify via User-Agent.
    let cfg = RateLimitConfig::nominatim();
    assert_eq!(cfg.requests_per_second, 1.0);
    assert_eq!(cfg.burst_size, 1);
    assert!(cfg.daily_quota.is_none());
    assert!(matches!(
        cfg.backoff_strategy,
        BackoffStrategy::Exponential { .. }
    ));
}

// ---------------------------------------------------------------------------
// Backoff delay progression (pure)
// ---------------------------------------------------------------------------

#[test]
fn exponential_backoff_progresses_then_caps() {
    let s = BackoffStrategy::Exponential {
        base: Duration::from_millis(10),
        max: Duration::from_millis(100),
        jitter: Duration::ZERO,
    };
    assert_eq!(s.delay(1), Duration::from_millis(10));
    assert_eq!(s.delay(2), Duration::from_millis(20));
    assert_eq!(s.delay(4), Duration::from_millis(80));
    assert_eq!(s.delay(5), Duration::from_millis(100)); // 160 capped
    assert_eq!(s.delay(50), Duration::from_millis(100)); // stays capped
}

#[test]
fn linear_backoff_progresses_then_caps() {
    let s = BackoffStrategy::Linear {
        base: Duration::from_millis(10),
        step: Duration::from_millis(10),
        max: Duration::from_millis(50),
        jitter: Duration::ZERO,
    };
    assert_eq!(s.delay(1), Duration::from_millis(10));
    assert_eq!(s.delay(2), Duration::from_millis(20));
    assert_eq!(s.delay(5), Duration::from_millis(50)); // 50, at cap
    assert_eq!(s.delay(6), Duration::from_millis(50)); // capped
}

#[test]
fn fixed_backoff_is_constant() {
    let s = BackoffStrategy::Fixed {
        delay: Duration::from_millis(25),
        jitter: Duration::ZERO,
    };
    assert_eq!(s.delay(1), Duration::from_millis(25));
    assert_eq!(s.delay(7), Duration::from_millis(25));
}

// ---------------------------------------------------------------------------
// Status classification
// ---------------------------------------------------------------------------

#[test]
fn is_retryable_status_matches_transient_set() {
    for ok in [429u16, 502, 503, 504] {
        assert!(is_retryable_status(ok), "{ok} should be retryable");
    }
    for bad in [200u16, 301, 400, 401, 403, 404, 408, 500, 599] {
        assert!(!is_retryable_status(bad), "{bad} should not be retryable");
    }
}

#[test]
fn retry_hint_from_status_carries_no_retry_after_by_default() {
    assert!(matches!(
        RetryHint::from_status(503, None),
        RetryHint::Retry { retry_after: None }
    ));
    assert!(matches!(RetryHint::from_status(200, None), RetryHint::Stop));
}

// ---------------------------------------------------------------------------
// Config validation
// ---------------------------------------------------------------------------

#[test]
fn rate_limiter_rejects_invalid_config() {
    let bad_rps = RateLimitConfig {
        requests_per_second: 0.0,
        burst_size: 1,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::default(),
    };
    assert!(matches!(
        RateLimiter::new(bad_rps).unwrap_err(),
        RateLimitError::InvalidConfig(_)
    ));

    let nan_rps = RateLimitConfig {
        requests_per_second: f32::NAN,
        burst_size: 1,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::default(),
    };
    assert!(matches!(
        RateLimiter::new(nan_rps).unwrap_err(),
        RateLimitError::InvalidConfig(_)
    ));

    let zero_burst = RateLimitConfig {
        requests_per_second: 1.0,
        burst_size: 0,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::default(),
    };
    assert!(matches!(
        RateLimiter::new(zero_burst).unwrap_err(),
        RateLimitError::InvalidConfig(_)
    ));

    let zero_quota = RateLimitConfig {
        requests_per_second: 1.0,
        burst_size: 1,
        daily_quota: Some(0),
        backoff_strategy: BackoffStrategy::default(),
    };
    assert!(matches!(
        RateLimiter::new(zero_quota).unwrap_err(),
        RateLimitError::InvalidConfig(_)
    ));
}

// ---------------------------------------------------------------------------
// Token-bucket throttling (timing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_bucket_allows_burst_then_throttles() {
    // 5 rps, burst 2 → first two acquires are immediate, the third must wait
    // ~200ms (one replenish interval) before governor admits it.
    let limiter = RateLimiter::new(RateLimitConfig {
        requests_per_second: 5.0,
        burst_size: 2,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::default(),
    })
    .unwrap();

    let start = tokio::time::Instant::now();
    limiter.acquire().await.unwrap();
    limiter.acquire().await.unwrap();
    let after_burst = start.elapsed();
    assert!(
        after_burst < Duration::from_millis(80),
        "burst of {after_burst:?} should be near-instant"
    );

    let throttle_start = tokio::time::Instant::now();
    limiter.acquire().await.unwrap();
    let waited = throttle_start.elapsed();
    assert!(
        waited >= Duration::from_millis(120) && waited < Duration::from_millis(1500),
        "third acquire should be throttled (~200ms), waited {waited:?}"
    );
}

// ---------------------------------------------------------------------------
// Daily quota exhaustion (public surface, 24h window)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn daily_quota_exhaustion_reports_resets_at() {
    let limiter = RateLimiter::new(RateLimitConfig {
        requests_per_second: 100.0,
        burst_size: 10,
        daily_quota: Some(1),
        backoff_strategy: BackoffStrategy::default(),
    })
    .unwrap();

    limiter.acquire().await.unwrap(); // first allowed
    let before = Utc::now();
    let err = limiter.acquire().await.unwrap_err();
    let after = Utc::now();

    match err {
        RateLimitError::QuotaExhausted { resets_at } => {
            // Rolling 24h window from the first request.
            assert!(
                resets_at > before && resets_at < after + Duration::from_secs(86_400 + 60),
                "resets_at {resets_at} should be ~24h after {before}"
            );
        }
        other => panic!("expected QuotaExhausted, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// retry_with_backoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_succeeds_after_transient_failures() {
    let calls = Arc::new(AtomicU32::new(0));
    let op = flaky_op(2, TestErr::Transient, 42u32, Arc::clone(&calls));
    let strategy = BackoffStrategy::Fixed {
        delay: Duration::from_millis(1),
        jitter: Duration::ZERO,
    };
    let result = retry_with_backoff(&strategy, 5, op).await.unwrap();
    assert_eq!(result, 42);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_exhausts_after_max_attempts() {
    let calls = Arc::new(AtomicU32::new(0));
    let op = flaky_op(u32::MAX, TestErr::Status(503), (), Arc::clone(&calls));
    let strategy = BackoffStrategy::Fixed {
        delay: Duration::from_millis(1),
        jitter: Duration::ZERO,
    };
    let err = retry_with_backoff(&strategy, 3, op).await.unwrap_err();
    match err {
        RetryError::Exhausted { attempts, error } => {
            assert_eq!(attempts, 3);
            assert_eq!(error, TestErr::Status(503));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_stops_immediately_on_non_retryable() {
    let calls = Arc::new(AtomicU32::new(0));
    let op = flaky_op(u32::MAX, TestErr::Terminal, (), Arc::clone(&calls));
    let strategy = BackoffStrategy::Fixed {
        delay: Duration::from_millis(1),
        jitter: Duration::ZERO,
    };
    let err = retry_with_backoff(&strategy, 5, op).await.unwrap_err();
    assert!(matches!(err, RetryError::Terminal(TestErr::Terminal)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_honours_server_retry_after() {
    // Server says wait 60ms; the backoff floor is only 1ms. The actual delay
    // must be driven by Retry-After, not the (smaller) backoff floor.
    let calls = Arc::new(AtomicU32::new(0));
    let calls_cl = Arc::clone(&calls);
    let op = move |_a: u32| {
        let n = calls_cl.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { if n <= 1 { Err(RetryAfterErr) } else { Ok(7u32) } })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<u32, RetryAfterErr>> + Send>,
            >
    };
    let strategy = BackoffStrategy::Fixed {
        delay: Duration::from_millis(1),
        jitter: Duration::ZERO,
    };
    let start = tokio::time::Instant::now();
    let val = retry_with_backoff(&strategy, 5, op).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(val, 7);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        elapsed >= Duration::from_millis(55),
        "should wait ~60ms for Retry-After, took {elapsed:?}"
    );
    assert!(elapsed < Duration::from_millis(2000));
}

#[tokio::test]
async fn retry_after_is_clamped_to_strategy_max() {
    // An unreasonable server hint (10s) must not stall the task: it is clamped
    // to the strategy's max cap (100ms), so the actual retry wait stays bounded.
    let calls = Arc::new(AtomicU32::new(0));
    let calls_cl = Arc::clone(&calls);
    let op = move |_a: u32| {
        let n = calls_cl.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if n <= 1 {
                Err(HugeRetryAfterErr)
            } else {
                Ok(9u32)
            }
        })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<u32, HugeRetryAfterErr>> + Send>,
            >
    };
    let strategy = BackoffStrategy::Exponential {
        base: Duration::from_millis(1),
        max: Duration::from_millis(100),
        jitter: Duration::ZERO,
    };
    let start = tokio::time::Instant::now();
    let val = retry_with_backoff(&strategy, 5, op).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(val, 9);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        elapsed >= Duration::from_millis(90),
        "should wait ~100ms (clamped), took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "unreasonable Retry-After leaked through unclamped: {elapsed:?}"
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HugeRetryAfterErr;

impl Retryable for HugeRetryAfterErr {
    fn retry_hint(&self) -> RetryHint {
        RetryHint::Retry {
            retry_after: Some(Duration::from_secs(10)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetryAfterErr;

impl Retryable for RetryAfterErr {
    fn retry_hint(&self) -> RetryHint {
        RetryHint::Retry {
            retry_after: Some(Duration::from_millis(60)),
        }
    }
}
