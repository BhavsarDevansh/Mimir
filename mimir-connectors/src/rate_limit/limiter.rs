//! The token-bucket limiter facade with optional daily quota.

use std::fmt;
use std::num::NonZeroU32;
use std::time::Duration;

use chrono::Utc;
use governor::Quota;

use crate::rate_limit::DAILY_WINDOW;
use crate::rate_limit::config::RateLimitConfig;
use crate::rate_limit::error::RateLimitError;
use crate::rate_limit::quota::{QuotaSnapshot, QuotaTracker};

type GovernorLimiter = governor::DefaultDirectRateLimiter;

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
