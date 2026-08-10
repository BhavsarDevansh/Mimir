//! Shared rate-limiting + retry/backoff primitives for network connectors
//! (Phase 3 F12 / issue #189).
//!
//! These primitives are intended for every connector that makes outbound
//! HTTP / IMAP / CalDAV API calls, so that throttling, daily quota
//! enforcement, and 429/503 retry behaviour are uniform across backends. They
//! are available infrastructure now; connectors adopt them as their backends
//! are implemented in later Phase 3 issues. **Connector LLM calls are exempt**
//! (decision D′ of the Phase 3 plan): those route through the shared
//! `LlmWorkerPool` system queue and must *not* be wrapped here — this limiter
//! governs service API calls only.
//!
//! # Design
//!
//! - **Token bucket** — [`RateLimiter::acquire`] blocks until the configured
//!   `requests_per_second` / `burst_size` admit a request. Backed by `governor`
//!   (a vetted, `unsafe`-free GCRA implementation).
//! - **Daily quota** — an optional rolling 24h cap layered on top of the token
//!   bucket. When exhausted, [`RateLimiter::acquire`] returns
//!   [`RateLimitError::QuotaExhausted`] (with `resets_at`) instead of parking a
//!   task for up to 24h, so the supervisor can pause the cycle gracefully.
//! - **Retry / backoff** — [`retry_with_backoff`] wraps an async operation and
//!   retries it on retryable errors (429/502/503/504, or any error the
//!   operation flags via [`Retryable`]) using the configured
//!   [`BackoffStrategy`] with jitter, honouring a server-supplied `Retry-After`
//!   when present.
//!
//! # Construction
//!
//! A [`RateLimiter`] is built per connector instance from its
//! [`RateLimitConfig`]. The config is `serde`-serialisable (durations use
//! human-readable strings via `humantime`, e.g. `"500ms"`, `"30s"`) so it can
//! embed directly in a connector's `config_json`. [`RateLimitConfig::nominatim`]
//! ships a policy-compliant preset for the OSM Nominatim geocoder (≤ 1 req/s).
//!
//! # Module layout
//!
//! - `config` — backoff strategy, rate-limit config, serde helpers.
//! - `error` — limiter errors.
//! - `quota` — rolling daily-quota tracker.
//! - `limiter` — token-bucket facade.
//! - `retry` — retry/backoff primitives.

use std::time::Duration;

mod config;
mod error;
mod limiter;
mod quota;
mod retry;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Rolling window used by the daily-quota tracker: 24h from the first request
/// in the current window.
const DAILY_WINDOW: Duration = Duration::from_secs(86_400);

/// Fallback ceiling for a server-supplied `Retry-After` when the backoff
/// strategy has no explicit `max` (i.e. [`BackoffStrategy::Fixed`]). Bounds an
/// unreasonable server hint so it cannot stall a connector task for an
/// unbounded duration; connectors wanting a different ceiling should use an
/// `Exponential`/`Linear` strategy with their own `max`.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

pub use config::{BackoffStrategy, RateLimitConfig};
pub use error::RateLimitError;
pub use limiter::RateLimiter;
pub use quota::QuotaSnapshot;
pub use retry::{RetryError, RetryHint, Retryable, is_retryable_status, retry_with_backoff};
