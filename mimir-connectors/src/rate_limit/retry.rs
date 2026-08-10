//! Retry / backoff primitives: retryable errors, jittered delays, `Retry-After`.

use std::fmt;
use std::time::Duration;

use crate::rate_limit::MAX_RETRY_AFTER;
use crate::rate_limit::config::BackoffStrategy;

/// Classification of an operation error for [`retry_with_backoff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryHint {
    /// Retry the operation. `retry_after` overrides the computed backoff delay
    /// when set (e.g. parsed from an HTTP `Retry-After` header).
    Retry { retry_after: Option<Duration> },
    /// Do not retry; surface the error immediately.
    Stop,
}

impl RetryHint {
    /// Classify an HTTP status code. The retryable set is `{429, 502, 503,
    /// 504}` (matching the shared `LlmClient` transient classification); any
    /// `retry_after` is carried through for the caller to honour.
    pub fn from_status(status: u16, retry_after: Option<Duration>) -> Self {
        if is_retryable_status(status) {
            Self::Retry { retry_after }
        } else {
            Self::Stop
        }
    }
}

pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

/// Trait implemented by error types that can tell [`retry_with_backoff`] whether
/// (and how long) to retry.
///
/// Connector backends implement this on their request-error enum; the HTTP
/// case can delegate to [`RetryHint::from_status`] with a parsed `Retry-After`.
pub trait Retryable {
    /// Whether to retry, and an optional server-supplied `Retry-After`.
    fn retry_hint(&self) -> RetryHint;
}

/// Error returned by [`retry_with_backoff`].
#[derive(Debug, thiserror::Error)]
pub enum RetryError<E> {
    /// The operation kept failing with retryable errors until `max_attempts`
    /// was reached.
    #[error("retry exhausted after {attempts} attempt(s): {error:?}")]
    Exhausted { attempts: u32, error: E },
    /// The operation failed with a non-retryable error on the first attempt.
    #[error("non-retryable error: {0:?}")]
    Terminal(E),
}

/// Retry `operation` with backoff per `strategy`, up to `max_attempts` total
/// attempts (the first call counts as attempt 1).
///
/// On each error, the operation's [`Retryable::retry_hint`] decides whether to
/// retry. The delay before a retry is the strategy's [`BackoffStrategy::delay`]
/// for the failed attempt, unless the error supplied a `retry_after` (e.g. an
/// HTTP `Retry-After` header), in which case that value is honoured but clamped
/// to the strategy's [`BackoffStrategy::max_cap`] (or `MAX_RETRY_AFTER` when
/// the strategy has no `max`) so an unreasonable server hint cannot stall the
/// connector beyond the configured ceiling; the strategy's jitter is then
/// added uniformly in `[0, jitter]`.
///
/// `max_attempts` is clamped to a minimum of 1. A non-retryable error returns
/// [`RetryError::Terminal`] immediately; exhausting the budget returns
/// [`RetryError::Exhausted`] with the last error and the attempt count. The
/// error is exposed as the `error` field (not `source`, since `E` is only required
pub async fn retry_with_backoff<E, F, Fut, T>(
    strategy: &BackoffStrategy,
    max_attempts: u32,
    mut operation: F,
) -> Result<T, RetryError<E>>
where
    E: fmt::Debug + Retryable,
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let cap = max_attempts.max(1);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match operation(attempt).await {
            Ok(value) => return Ok(value),
            Err(error) => match error.retry_hint() {
                RetryHint::Stop => return Err(RetryError::Terminal(error)),
                RetryHint::Retry { retry_after } => {
                    if attempt >= cap {
                        return Err(RetryError::Exhausted {
                            attempts: attempt,
                            error,
                        });
                    }
                    let delay = retry_delay_with_jitter(strategy, attempt, retry_after);
                    tokio::time::sleep(delay).await;
                }
            },
        }
    }
}

/// Resolve the sleep delay for a retry attempt (before jitter).
///
/// A server-supplied `retry_after` (e.g. parsed from an HTTP `Retry-After`
/// header) is honoured, but clamped to the strategy's [`BackoffStrategy::max_cap`]
/// (or [`MAX_RETRY_AFTER`] when the strategy has no `max`) so an unreasonable
/// hint cannot stall a connector task beyond the configured ceiling. When no
/// `retry_after` is supplied the strategy's computed backoff is used (already
/// capped by `max` inside [`BackoffStrategy::delay`]). Exposed so the clamping
pub(super) fn retry_delay(
    strategy: &BackoffStrategy,
    attempt: u32,
    retry_after: Option<Duration>,
) -> Duration {
    match retry_after {
        Some(ra) => ra.min(strategy.max_cap().unwrap_or(MAX_RETRY_AFTER)),
        None => strategy.delay(attempt),
    }
}

/// Compute the final retry sleep delay: the [`retry_delay`] base, plus uniform
/// jitter, then clamped back to the strategy's bound so the jittered result
/// never exceeds the bounded-delay contract.
///
/// For `Exponential`/`Linear` (which expose a `max_cap`) the jittered delay is
/// always clamped to that `max`. For `Fixed` (no `max_cap`) a server-supplied
/// `Retry-After` is clamped to [`MAX_RETRY_AFTER`] after jitter; a `Fixed`
/// delay with no server hint is the caller's explicit configured value and is
/// left unbounded. Exposed so the clamp-after-jitter is unit-testable without
pub(super) fn retry_delay_with_jitter(
    strategy: &BackoffStrategy,
    attempt: u32,
    retry_after: Option<Duration>,
) -> Duration {
    let base = retry_delay(strategy, attempt, retry_after);
    let jittered = apply_jitter(base, strategy.jitter());
    match strategy.max_cap() {
        Some(cap) => jittered.min(cap),
        None => match retry_after {
            Some(_) => jittered.min(MAX_RETRY_AFTER),
            None => jittered,
        },
    }
}

/// Add a uniform random amount in `[0, jitter]` to `base`. No-op when `jitter`
pub(super) fn apply_jitter(base: Duration, jitter: Duration) -> Duration {
    if jitter.is_zero() {
        return base;
    }
    use rand::Rng;
    let max_nanos = u64::try_from(jitter.as_nanos()).unwrap_or(u64::MAX);
    let extra = rand::rng().random_range(0..max_nanos.saturating_add(1));
    // `saturating_add` keeps the jittered delay from panicking on overflow.
    base.saturating_add(Duration::from_nanos(extra))
}
