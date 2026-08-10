//! Errors returned by the rate limiter.

use chrono::{DateTime, Utc};

/// Errors raised by [`RateLimiter`](crate::rate_limit::RateLimiter) construction or admission.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RateLimitError {
    /// The [`RateLimitConfig`](crate::rate_limit::RateLimitConfig) was invalid (non-positive rate, zero burst,
    /// zero quota, or a rate too large to represent as a replenish interval).
    #[error("invalid rate-limit config: {0}")]
    InvalidConfig(String),

    /// A persisted [`QuotaSnapshot`](crate::rate_limit::QuotaSnapshot) supplied to
    /// [`RateLimiter::with_quota_state`](crate::rate_limit::RateLimiter::with_quota_state) was invalid — for example its
    /// `window_start` plus the configured window would overflow
    /// `DateTime<Utc>`, which would otherwise panic during admission. Repair
    /// or discard the snapshot before reconstructing the limiter.
    #[error("invalid quota snapshot: {0}")]
    InvalidSnapshot(String),

    /// The rolling 24h daily quota has been spent. `resets_at` is when the
    /// window rolls over and requests resume. The caller (connector /
    /// supervisor) should treat this as non-retryable for the current cycle
    /// rather than blocking a task until `resets_at`.
    #[error("daily quota exhausted; resets at {resets_at}")]
    QuotaExhausted { resets_at: DateTime<Utc> },
}
