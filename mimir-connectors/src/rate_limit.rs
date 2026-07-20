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

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use governor::Quota;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use tokio::sync::Mutex;

/// Rolling window used by the daily-quota tracker: 24h from the first request
/// in the current window.
const DAILY_WINDOW: Duration = Duration::from_secs(86_400);

/// Fallback ceiling for a server-supplied `Retry-After` when the backoff
/// strategy has no explicit `max` (i.e. [`BackoffStrategy::Fixed`]). Bounds an
/// unreasonable server hint so it cannot stall a connector task for an
/// unbounded duration; connectors wanting a different ceiling should use an
/// `Exponential`/`Linear` strategy with their own `max`.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// Duration serde helper (human-readable via humantime)
// ---------------------------------------------------------------------------

mod duration_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&humantime::format_duration(*value))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        humantime::parse_duration(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Backoff strategy
// ---------------------------------------------------------------------------

/// How [`retry_with_backoff`] spaces successive retry attempts.
///
/// All variants carry a `jitter` budget added (uniformly, in `[0, jitter]`) on
/// top of the computed delay to de-synchronize retries across connector
/// instances and avoid thundering-herd spikes against a recovering service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackoffStrategy {
    /// `min(base * 2^(attempt-1), max)` per attempt.
    Exponential {
        #[serde(with = "duration_serde")]
        base: Duration,
        #[serde(with = "duration_serde")]
        max: Duration,
        #[serde(with = "duration_serde")]
        jitter: Duration,
    },
    /// `min(base + step * (attempt-1), max)` per attempt.
    Linear {
        #[serde(with = "duration_serde")]
        base: Duration,
        #[serde(with = "duration_serde")]
        step: Duration,
        #[serde(with = "duration_serde")]
        max: Duration,
        #[serde(with = "duration_serde")]
        jitter: Duration,
    },
    /// A constant `delay` per attempt.
    Fixed {
        #[serde(with = "duration_serde")]
        delay: Duration,
        #[serde(with = "duration_serde")]
        jitter: Duration,
    },
}

impl Default for BackoffStrategy {
    /// A conservative exponential default: 1s → 60s with up to 250ms jitter.
    fn default() -> Self {
        Self::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            jitter: Duration::from_millis(250),
        }
    }
}

impl BackoffStrategy {
    /// Base delay for a 1-based `attempt`, before jitter, capped at `max`.
    ///
    /// `attempt` is clamped to a minimum of 1 and the growth exponent to 31 to
    /// avoid overflow; the cap then dominates for large attempt counts.
    pub fn delay(&self, attempt: u32) -> Duration {
        let a = attempt.max(1);
        match self {
            Self::Exponential { base, max, .. } => {
                let exponent = (a - 1).min(31);
                base.checked_mul(2u32.saturating_pow(exponent))
                    .unwrap_or(*max)
                    .min(*max)
            }
            Self::Linear {
                base, step, max, ..
            } => {
                let grown = step.saturating_mul(a - 1);
                // `saturating_add` avoids a panic when config-derived `base`
                // and `grown` overflow `Duration` before the `max` clamp.
                base.saturating_add(grown).min(*max)
            }
            Self::Fixed { delay, .. } => *delay,
        }
    }

    /// Jitter budget for this strategy, applied by [`retry_with_backoff`].
    pub fn jitter(&self) -> Duration {
        match self {
            Self::Exponential { jitter, .. }
            | Self::Linear { jitter, .. }
            | Self::Fixed { jitter, .. } => *jitter,
        }
    }

    /// Upper bound on a single retry wait, used to clamp a server-supplied
    /// `Retry-After`. `Exponential` and `Linear` expose their configured `max`;
    /// `Fixed` has none and falls back to [`MAX_RETRY_AFTER`].
    pub fn max_cap(&self) -> Option<Duration> {
        match self {
            Self::Exponential { max, .. } | Self::Linear { max, .. } => Some(*max),
            Self::Fixed { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for a connector's outbound API rate limiting + retry policy.
///
/// One of these is embedded per connector instance (in its `config_json`) and
/// used to build a [`RateLimiter`]. Fields match the Phase 3 F12 spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Sustained request rate. Fractional values are supported (e.g. `0.5` =
    /// one request every two seconds). Must be a positive, finite number.
    pub requests_per_second: f32,
    /// Maximum requests dispatched in an instantaneous burst before the
    /// sustained rate kicks in. Must be at least 1.
    pub burst_size: u32,
    /// Optional cap on total requests per rolling 24h window. `None` disables
    /// the daily quota.
    pub daily_quota: Option<u32>,
    /// Retry spacing used by [`retry_with_backoff`] for 429/503-class errors.
    pub backoff_strategy: BackoffStrategy,
}

impl Default for RateLimitConfig {
    /// A conservative default: 1 req/s, burst 1, no daily quota, exponential
    /// backoff. Safe for most public APIs; tighten per service as needed.
    fn default() -> Self {
        Self {
            requests_per_second: 1.0,
            burst_size: 1,
            daily_quota: None,
            backoff_strategy: BackoffStrategy::default(),
        }
    }
}

impl RateLimitConfig {
    /// Preset compliant with the OSM Nominatim usage policy: ≤ 1 req/s, no
    /// burst, no daily quota, exponential backoff. The caller is still
    /// responsible for sending an identifying `User-Agent`.
    pub fn nominatim() -> Self {
        Self {
            requests_per_second: 1.0,
            burst_size: 1,
            daily_quota: None,
            backoff_strategy: BackoffStrategy::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors raised by [`RateLimiter`] construction or admission.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RateLimitError {
    /// The [`RateLimitConfig`] was invalid (non-positive rate, zero burst,
    /// zero quota, or a rate too large to represent as a replenish interval).
    #[error("invalid rate-limit config: {0}")]
    InvalidConfig(String),

    /// A persisted [`QuotaSnapshot`] supplied to
    /// [`RateLimiter::with_quota_state`] was invalid — for example its
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

// ---------------------------------------------------------------------------
// Daily-quota tracker (private, clock-injectable for unit tests)
// ---------------------------------------------------------------------------

/// Serializable snapshot of a [`RateLimiter`]'s rolling daily-quota window,
/// for persisting across daemon/connector restarts.
///
/// A reconstructed [`RateLimiter`] built via [`RateLimiter::with_quota_state`]
/// resumes the saved `count` / `window_start` instead of resetting the daily
/// allowance to zero, so a restart cannot silently bypass a provider's hard
/// 24-hour quota. Read it with [`RateLimiter::quota_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    /// Requests already consumed in the current window.
    pub count: u32,
    /// When the current rolling window started (UTC).
    pub window_start: DateTime<Utc>,
    /// Monotonic version that increases on every successful
    /// `check_and_increment` (and is carried across reconstruction). A
    /// persistence layer MUST treat `version` as the compare-and-swap guard
    /// for a window: only overwrite a previously persisted snapshot whose
    /// `version` is strictly lower, and never persist a snapshot before its
    /// corresponding `acquire` has been admitted, so a delayed out-of-order
    /// write (e.g. count 1 landing after count 2) cannot revive a stale,
    /// lower count after a restart. Defaults to `0` for snapshots authored
    /// before this field existed.
    #[serde(default)]
    pub version: u64,
}

#[derive(Debug)]
struct QuotaState {
    count: u32,
    window_start: DateTime<Utc>,
    /// Monotonic version bumped on every successful `check_and_increment` and
    /// carried across reconstruction via [`QuotaSnapshot::version`], so a
    /// persistence layer can reject regressive out-of-order writes.
    version: u64,
}

impl From<&QuotaState> for QuotaSnapshot {
    fn from(state: &QuotaState) -> Self {
        Self {
            count: state.count,
            window_start: state.window_start,
            version: state.version,
        }
    }
}

#[derive(Debug)]
struct QuotaTracker {
    quota: u32,
    window: TimeDelta,
    state: Mutex<QuotaState>,
}

impl QuotaTracker {
    fn new(quota: u32, window: Duration) -> Self {
        // `from_std` only fails for negative or > i64::max durations, neither
        // of which the 24h constant (or any test value) can be.
        let window = TimeDelta::from_std(window).expect("quota window fits in TimeDelta");
        Self {
            quota,
            window,
            state: Mutex::new(QuotaState {
                count: 0,
                // Epoch sentinel: the first `check_and_increment` always
                // observes `now >= window_start + window` and resets the
                // window to `now`, so the window starts on first use.
                window_start: DateTime::from_timestamp(0, 0).expect("epoch is valid"),
                version: 0,
            }),
        }
    }

    /// Build a tracker that resumes a previously persisted quota window
    /// ([`QuotaSnapshot`]) instead of starting from zero. A snapshot whose
    /// window has already elapsed behaves like a fresh tracker: the first
    /// `check_and_increment` resets the window to `now`.
    fn with_snapshot(
        quota: u32,
        window: Duration,
        snapshot: QuotaSnapshot,
    ) -> Result<Self, RateLimitError> {
        let window = TimeDelta::from_std(window).expect("quota window fits in TimeDelta");
        // `QuotaSnapshot` is public and `serde`-deserialisable, so a crafted
        // `window_start` near `DateTime::<Utc>::MAX_UTC` could make the
        // in-memory window-end arithmetic (`window_start + window`) panic in
        // `is_exhausted` / `check_and_increment`. Validate the restored
        // window end up front and reject snapshots that would overflow
        // instead of constructing a state that can panic later.
        if snapshot.window_start.checked_add_signed(window).is_none() {
            return Err(RateLimitError::InvalidSnapshot(format!(
                "window_start ({}) + window ({}s) overflows DateTime<Utc>",
                snapshot.window_start,
                window.num_seconds()
            )));
        }
        Ok(Self {
            quota,
            window,
            state: Mutex::new(QuotaState {
                count: snapshot.count,
                window_start: snapshot.window_start,
                version: snapshot.version,
            }),
        })
    }

    /// Read the current window state for persistence.
    async fn snapshot(&self) -> QuotaSnapshot {
        let state = self.state.lock().await;
        QuotaSnapshot::from(&*state)
    }

    /// Whether the quota is already spent at `now`, returning the `resets_at`
    /// timestamp when it is. Used as a fail-fast pre-check in
    /// [`RateLimiter::acquire`] so an exhausted quota is reported without first
    /// parking on the token bucket (which, for low rates, can be a long wait).
    /// A window that has already elapsed returns `None` (it will roll over on
    /// the next `check_and_increment`).
    async fn is_exhausted(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let state = self.state.lock().await;
        if now >= state.window_start + self.window {
            return None;
        }
        if state.count >= self.quota {
            Some(state.window_start + self.window)
        } else {
            None
        }
    }

    /// Increment the window counter, resetting the window first if it has
    /// elapsed. Returns `Err(QuotaExhausted)` (with `resets_at`) when the
    /// quota is spent.
    async fn check_and_increment(&self, now: DateTime<Utc>) -> Result<(), RateLimitError> {
        let mut state = self.state.lock().await;
        if now >= state.window_start + self.window {
            state.window_start = now;
            state.count = 0;
        }
        if state.count >= self.quota {
            return Err(RateLimitError::QuotaExhausted {
                resets_at: state.window_start + self.window,
            });
        }
        state.count += 1;
        state.version = state.version.saturating_add(1);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

type GovernorLimiter = governor::DefaultDirectRateLimiter;

/// Token-bucket + daily-quota admission gate for one connector's outbound API
/// calls.
///
/// Construct one per connector instance via [`RateLimiter::new`], then call
/// [`RateLimiter::acquire`][Self::acquire] before every outbound request.
/// `acquire` fail-fasts on an exhausted daily quota, awaits the token bucket
/// (throttling to `requests_per_second` / `burst_size`), and then atomically
/// checks and increments the optional daily quota. LLM calls must not go
/// through this gate (decision D′).
///
/// The limiter is `Send + Sync` and cheap to hold behind an `Arc`; it is *not*
/// `Clone` because the underlying GCRA state is shared and mutable.
pub struct RateLimiter {
    config: RateLimitConfig,
    inner: GovernorLimiter,
    quota: Option<QuotaTracker>,
}

impl RateLimiter {
    /// Build a limiter from its config, validating the rate / burst / quota.
    ///
    /// Equivalent to [`RateLimiter::with_quota_state`] with no prior quota
    /// state — the daily quota window starts fresh on first use.
    pub fn new(config: RateLimitConfig) -> Result<Self, RateLimitError> {
        Self::with_quota_state(config, None)
    }

    /// Build a limiter from its config, optionally restoring a persisted
    /// daily-quota window ([`QuotaSnapshot`]).
    ///
    /// When `quota_state` is `Some` and the config enables a `daily_quota`,
    /// the reconstructed limiter resumes the saved `count` / `window_start`
    /// instead of resetting the allowance to zero, so a daemon or connector
    /// restart cannot silently exceed a provider's hard 24-hour quota. A
    /// `quota_state` supplied when `daily_quota` is `None`, or whose window has
    /// already elapsed, is harmlessly ignored (the latter rolls over on first
    /// use). Validation is the same as [`RateLimiter::new`].
    pub fn with_quota_state(
        config: RateLimitConfig,
        quota_state: Option<QuotaSnapshot>,
    ) -> Result<Self, RateLimitError> {
        validate(&config)?;

        // GCRA: one cell replenished every `1/rps` seconds, with an
        // independent burst capacity. `with_period` returns `None` for a
        // zero period (a finite-but-too-large rate whose reciprocal rounds to
        // a zero duration), which we surface as an invalid config.
        let period = Duration::from_secs_f32(1.0 / config.requests_per_second);
        let quota = Quota::with_period(period)
            .ok_or_else(|| {
                RateLimitError::InvalidConfig(
                    "requests_per_second is too large to represent a replenish interval".into(),
                )
            })?
            .allow_burst(NonZeroU32::new(config.burst_size).expect("burst_size validated >= 1"));
        let inner = governor::RateLimiter::direct(quota);

        let tracker = match config.daily_quota {
            Some(daily) => match quota_state {
                Some(snapshot) => Some(QuotaTracker::with_snapshot(daily, DAILY_WINDOW, snapshot)?),
                None => Some(QuotaTracker::new(daily, DAILY_WINDOW)),
            },
            None => None,
        };

        Ok(Self {
            config,
            inner,
            quota: tracker,
        })
    }

    /// The config this limiter was built from.
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// Wait until the token bucket admits a request and the daily quota has
    /// not been spent.
    ///
    /// First fail-fasts on a known-exhausted daily quota (returning
    /// [`RateLimitError::QuotaExhausted`] without sleeping), then blocks on the
    /// GCRA token bucket (governor handles its own clock), then atomically
    /// checks and increments the quota. The pre-check avoids parking for a
    /// full replenish interval — which, for low configured rates, can be hours
    /// — before reporting an already-known exhaustion. The authoritative
    /// increment still happens after token admission so concurrent acquires
    /// cannot overshoot the quota.
    pub async fn acquire(&self) -> Result<(), RateLimitError> {
        if let Some(tracker) = &self.quota {
            let now = Utc::now();
            if let Some(resets_at) = tracker.is_exhausted(now).await {
                return Err(RateLimitError::QuotaExhausted { resets_at });
            }
        }
        self.inner.until_ready().await;
        if let Some(tracker) = &self.quota {
            tracker.check_and_increment(Utc::now()).await?;
        }
        Ok(())
    }

    /// Snapshot the current daily-quota window state for persistence, or
    /// `None` when no daily quota is configured. Pass the result to
    /// [`RateLimiter::with_quota_state`] on reconstruction to resume the
    /// rolling window across daemon/connector restarts.
    pub async fn quota_snapshot(&self) -> Option<QuotaSnapshot> {
        match &self.quota {
            Some(tracker) => Some(tracker.snapshot().await),
            None => None,
        }
    }
}

impl fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimiter")
            .field("config", &self.config)
            .field("has_daily_quota", &self.quota.is_some())
            .finish_non_exhaustive()
    }
}

fn validate(config: &RateLimitConfig) -> Result<(), RateLimitError> {
    if !config.requests_per_second.is_finite() || config.requests_per_second <= 0.0 {
        return Err(RateLimitError::InvalidConfig(
            "requests_per_second must be a positive, finite number".into(),
        ));
    }
    if config.burst_size == 0 {
        return Err(RateLimitError::InvalidConfig(
            "burst_size must be at least 1".into(),
        ));
    }
    if let Some(quota) = config.daily_quota {
        if quota == 0 {
            return Err(RateLimitError::InvalidConfig(
                "daily_quota, when set, must be at least 1".into(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Retry / backoff layer
// ---------------------------------------------------------------------------

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

/// Whether an HTTP status code should be retried with backoff.
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
/// to the strategy's [`BackoffStrategy::max_cap`] (or [`MAX_RETRY_AFTER`] when
/// the strategy has no `max`) so an unreasonable server hint cannot stall the
/// connector beyond the configured ceiling; the strategy's jitter is then
/// added uniformly in `[0, jitter]`.
///
/// `max_attempts` is clamped to a minimum of 1. A non-retryable error returns
/// [`RetryError::Terminal`] immediately; exhausting the budget returns
/// [`RetryError::Exhausted`] with the last error and the attempt count. The
/// error is exposed as the `error` field (not `source`, since `E` is only required
/// to be `Debug`, not a `std::error::Error`).
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
/// is unit-testable without sleeping.
fn retry_delay(
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
/// sleeping.
fn retry_delay_with_jitter(
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
/// is zero (keeps retries deterministic in tests and when jitter is disabled).
fn apply_jitter(base: Duration, jitter: Duration) -> Duration {
    if jitter.is_zero() {
        return base;
    }
    use rand::Rng;
    let max_nanos = u64::try_from(jitter.as_nanos()).unwrap_or(u64::MAX);
    let extra = rand::rng().random_range(0..max_nanos.saturating_add(1));
    // `saturating_add` keeps the jittered delay from panicking on overflow.
    base.saturating_add(Duration::from_nanos(extra))
}

// ---------------------------------------------------------------------------
// Unit tests — pure logic with injected time / no async timing flakiness
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    #[tokio::test]
    async fn quota_tracker_allows_until_quota_then_exhausts() {
        let tracker = QuotaTracker::new(2, Duration::from_secs(60));
        let now = t(1000);
        assert!(tracker.check_and_increment(now).await.is_ok());
        assert!(tracker.check_and_increment(now).await.is_ok());
        let err = tracker.check_and_increment(now).await.unwrap_err();
        match err {
            RateLimitError::QuotaExhausted { resets_at } => {
                assert_eq!(resets_at, t(1060));
            }
            other => panic!("expected QuotaExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn quota_tracker_resets_after_window_elapses() {
        let tracker = QuotaTracker::new(1, Duration::from_secs(60));
        assert!(tracker.check_and_increment(t(1000)).await.is_ok());
        assert!(tracker.check_and_increment(t(1000)).await.is_err()); // exhausted
        // 61s later the window has rolled over → admission resumes.
        assert!(tracker.check_and_increment(t(1061)).await.is_ok());
    }

    #[test]
    fn backoff_delay_clamps_attempt_zero_to_one() {
        let s = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::ZERO,
        };
        assert_eq!(s.delay(0), Duration::from_millis(10));
    }

    #[test]
    fn backoff_exponential_does_not_overflow_for_huge_attempts() {
        let s = BackoffStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            jitter: Duration::ZERO,
        };
        assert_eq!(s.delay(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn max_cap_exposed_for_capped_strategies_none_for_fixed() {
        let exp = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::ZERO,
        };
        assert_eq!(exp.max_cap(), Some(Duration::from_millis(100)));

        let lin = BackoffStrategy::Linear {
            base: Duration::from_millis(10),
            step: Duration::from_millis(10),
            max: Duration::from_millis(50),
            jitter: Duration::ZERO,
        };
        assert_eq!(lin.max_cap(), Some(Duration::from_millis(50)));

        let fixed = BackoffStrategy::Fixed {
            delay: Duration::from_millis(25),
            jitter: Duration::ZERO,
        };
        assert_eq!(fixed.max_cap(), None);
    }

    #[test]
    fn retry_delay_honours_small_retry_after() {
        let s = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::ZERO,
        };
        // A server hint smaller than the cap is honoured as-is.
        assert_eq!(
            retry_delay(&s, 1, Some(Duration::from_millis(40))),
            Duration::from_millis(40)
        );
    }

    #[test]
    fn retry_delay_clamps_large_retry_after_to_strategy_max() {
        let s = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::ZERO,
        };
        // An unreasonable server hint is clamped to the strategy's max cap.
        assert_eq!(
            retry_delay(&s, 1, Some(Duration::from_secs(600))),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn retry_delay_falls_back_to_default_ceiling_for_fixed_strategy() {
        let s = BackoffStrategy::Fixed {
            delay: Duration::from_millis(25),
            jitter: Duration::ZERO,
        };
        // Fixed has no max_cap, so the default MAX_RETRY_AFTER ceiling applies.
        assert_eq!(
            retry_delay(&s, 1, Some(Duration::from_secs(600))),
            MAX_RETRY_AFTER
        );
        // A hint below the default ceiling is honoured.
        assert_eq!(
            retry_delay(&s, 1, Some(Duration::from_secs(10))),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn retry_delay_uses_computed_backoff_when_no_retry_after() {
        let s = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::ZERO,
        };
        assert_eq!(retry_delay(&s, 1, None), Duration::from_millis(10));
        assert_eq!(retry_delay(&s, 5, None), Duration::from_millis(100));
    }

    // --- Finding 3: saturating Duration arithmetic (no overflow panic) ---

    #[test]
    fn linear_delay_saturates_instead_of_panicking_near_duration_max() {
        let s = BackoffStrategy::Linear {
            base: Duration::MAX,
            step: Duration::from_secs(1),
            max: Duration::from_secs(1),
            jitter: Duration::ZERO,
        };
        // `base + grown` would overflow without `saturating_add`; the `max`
        // clamp must still dominate, yielding the configured cap.
        assert_eq!(s.delay(u32::MAX), Duration::from_secs(1));
    }

    #[test]
    fn linear_delay_saturates_to_duration_max_when_cap_is_max() {
        let s = BackoffStrategy::Linear {
            base: Duration::MAX,
            step: Duration::from_secs(1),
            max: Duration::MAX,
            jitter: Duration::ZERO,
        };
        assert_eq!(s.delay(u32::MAX), Duration::MAX);
    }

    #[test]
    fn apply_jitter_saturates_instead_of_panicking_near_duration_max() {
        // base + extra nanos overflows `Duration`; the result must saturate to
        // `Duration::MAX` rather than panicking, for any random draw.
        for _ in 0..128 {
            assert_eq!(apply_jitter(Duration::MAX, Duration::MAX), Duration::MAX);
        }
    }

    // --- Finding 6: clamp the jittered delay, not only its base ---

    #[test]
    fn retry_delay_with_jitter_respects_cap_when_jitter_is_zero() {
        let s = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::ZERO,
        };
        assert_eq!(
            retry_delay_with_jitter(&s, 1, None),
            Duration::from_millis(10)
        );
        assert_eq!(
            retry_delay_with_jitter(&s, 1, Some(Duration::from_secs(600))),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn retry_delay_with_jitter_never_exceeds_strategy_cap() {
        // A server hint at the cap plus any jitter in [0, 50ms] must remain
        // clamped to the 100ms cap (not 150ms), for every random draw.
        let s = BackoffStrategy::Exponential {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
            jitter: Duration::from_millis(50),
        };
        for _ in 0..256 {
            let delay = retry_delay_with_jitter(&s, 1, Some(Duration::from_millis(100)));
            assert!(
                delay <= Duration::from_millis(100),
                "jittered delay {delay:?} > cap"
            );
        }
    }

    #[test]
    fn retry_delay_with_jitter_bounds_fixed_strategy_server_hint() {
        // Fixed has no max_cap; a server hint at MAX_RETRY_AFTER plus jitter
        // must stay clamped to MAX_RETRY_AFTER, for every random draw.
        let s = BackoffStrategy::Fixed {
            delay: Duration::from_millis(25),
            jitter: Duration::from_secs(30),
        };
        for _ in 0..256 {
            let delay = retry_delay_with_jitter(&s, 1, Some(MAX_RETRY_AFTER));
            assert!(
                delay <= MAX_RETRY_AFTER,
                "jittered delay {delay:?} > MAX_RETRY_AFTER"
            );
        }
    }

    #[test]
    fn retry_delay_with_jitter_leaves_fixed_no_hint_unbounded() {
        // A configured Fixed delay with no server hint is the caller's explicit
        // choice and is not re-clamped to MAX_RETRY_AFTER.
        let s = BackoffStrategy::Fixed {
            delay: Duration::from_secs(400),
            jitter: Duration::ZERO,
        };
        assert_eq!(
            retry_delay_with_jitter(&s, 1, None),
            Duration::from_secs(400)
        );
    }

    // --- Finding 4: QuotaTracker snapshot/restore ---

    #[tokio::test]
    async fn quota_tracker_snapshot_round_trips_state() {
        let tracker = QuotaTracker::new(3, Duration::from_secs(60));
        let now = t(1000);
        tracker.check_and_increment(now).await.unwrap();
        tracker.check_and_increment(now).await.unwrap();
        let snap = tracker.snapshot().await;
        assert_eq!(snap.count, 2);
        assert_eq!(snap.window_start, now);

        let restored = QuotaTracker::with_snapshot(3, Duration::from_secs(60), snap).unwrap();
        // The third request is still allowed; the fourth exhausts.
        assert!(restored.check_and_increment(now).await.is_ok());
        let err = restored.check_and_increment(now).await.unwrap_err();
        match err {
            RateLimitError::QuotaExhausted { resets_at } => assert_eq!(resets_at, t(1060)),
            other => panic!("expected QuotaExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn quota_tracker_with_elapsed_snapshot_rolls_over() {
        // A snapshot whose window has already elapsed behaves like a fresh
        // tracker: the first check resets the window to `now`.
        let stale = QuotaSnapshot {
            count: 5,
            window_start: t(0),
            version: 0,
        };
        let tracker = QuotaTracker::with_snapshot(2, Duration::from_secs(60), stale).unwrap();
        assert!(tracker.check_and_increment(t(100_000)).await.is_ok());
    }

    #[tokio::test]
    async fn quota_tracker_is_exhausted_fail_fast() {
        let tracker = QuotaTracker::new(1, Duration::from_secs(60));
        let now = t(1000);
        assert!(tracker.check_and_increment(now).await.is_ok());
        // Quota spent: is_exhausted reports resets_at without incrementing.
        assert_eq!(tracker.is_exhausted(now).await, Some(t(1060)));
        // After the window elapses, it is no longer exhausted (rolls over).
        assert!(tracker.is_exhausted(t(1061)).await.is_none());
    }
}
