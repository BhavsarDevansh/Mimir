//! Rate-limit configuration: strategy, serde helpers, and presets.

use std::time::Duration;

use serde::{Deserialize, Serialize};

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

    /// Jitter budget for this strategy, applied by [`retry_with_backoff`](crate::rate_limit::retry_with_backoff).
    pub fn jitter(&self) -> Duration {
        match self {
            Self::Exponential { jitter, .. }
            | Self::Linear { jitter, .. }
            | Self::Fixed { jitter, .. } => *jitter,
        }
    }

    /// Upper bound on a single retry wait, used to clamp a server-supplied
    /// `Retry-After`. `Exponential` and `Linear` expose their configured `max`;
    /// `Fixed` has none and falls back to `MAX_RETRY_AFTER`.
    pub fn max_cap(&self) -> Option<Duration> {
        match self {
            Self::Exponential { max, .. } | Self::Linear { max, .. } => Some(*max),
            Self::Fixed { .. } => None,
        }
    }
}

/// One of these is embedded per connector instance (in its `config_json`) and
/// used to build a [`RateLimiter`](crate::rate_limit::RateLimiter). Fields match the Phase 3 F12 spec.
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
    /// Retry spacing used by [`retry_with_backoff`](crate::rate_limit::retry_with_backoff) for 429/503-class errors.
    pub backoff_strategy: BackoffStrategy,
}

impl Default for RateLimitConfig {
    /// A conservative default: 1 req/s, burst 1, no daily quota, exponential
    /// backoff. Safe for most public APIs; tighten per service as needed.
    /// This is also the [`RateLimitConfig::nominatim`] preset.
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
    /// responsible for sending an identifying `User-Agent`. This is exactly
    /// the conservative [`Default`] config, so the preset delegates to it to
    /// keep a single source of truth (issue #223).
    pub fn nominatim() -> Self {
        Self::default()
    }
}
