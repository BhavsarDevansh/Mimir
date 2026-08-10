//! Rolling daily-quota tracker layered on the token bucket.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::Mutex;

use crate::rate_limit::error::RateLimitError;

/// allowance to zero, so a restart cannot silently bypass a provider's hard
/// 24-hour quota. Read it with [`RateLimiter::quota_snapshot`](crate::rate_limit::RateLimiter::quota_snapshot).
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
pub(super) struct QuotaTracker {
    quota: u32,
    window: TimeDelta,
    state: Mutex<QuotaState>,
}

impl QuotaTracker {
    pub(super) fn new(quota: u32, window: Duration) -> Self {
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
    pub(super) fn with_snapshot(
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
    pub(super) async fn snapshot(&self) -> QuotaSnapshot {
        let state = self.state.lock().await;
        QuotaSnapshot::from(&*state)
    }

    /// Whether the quota is already spent at `now`, returning the `resets_at`
    /// timestamp when it is. Used as a fail-fast pre-check in
    /// [`RateLimiter::acquire`] so an exhausted quota is reported without first
    /// parking on the token bucket (which, for low rates, can be a long wait).
    /// A window that has already elapsed returns `None` (it will roll over on
    /// the next `check_and_increment`).
    pub(super) async fn is_exhausted(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
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
    pub(super) async fn check_and_increment(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(), RateLimitError> {
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
