use super::*;

use super::quota::QuotaTracker;
use super::retry::{apply_jitter, retry_delay, retry_delay_with_jitter};
use chrono::{DateTime, Utc};

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
