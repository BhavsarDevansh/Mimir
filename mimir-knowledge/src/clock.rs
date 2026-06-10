//! Clock abstraction for testable time.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::sync::atomic::{AtomicI64, Ordering};

/// Provides the current UTC timestamp.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn today(&self) -> NaiveDate {
        self.now().date_naive()
    }
}

/// Real wall-clock time.
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Deterministic fake clock for tests.
pub struct MockClock {
    seconds: AtomicI64,
    nanos: AtomicI64,
}

impl MockClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            seconds: AtomicI64::new(start.timestamp()),
            nanos: AtomicI64::new(start.timestamp_subsec_nanos() as i64),
        }
    }

    pub fn advance(&self, duration: Duration) {
        let secs = duration.num_seconds();
        let nanos = duration.num_nanoseconds().unwrap_or(0) - secs * 1_000_000_000;
        self.seconds.fetch_add(secs, Ordering::SeqCst);
        self.nanos.fetch_add(nanos, Ordering::SeqCst);
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(
            self.seconds.load(Ordering::SeqCst),
            self.nanos.load(Ordering::SeqCst) as u32,
        )
        .expect("valid timestamp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_clock_returns_seeded_time() {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::new(fixed);
        assert_eq!(clock.now(), fixed);
    }

    #[test]
    fn mock_clock_advance_shifts_forward() {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::new(fixed);
        clock.advance(Duration::days(7));
        assert_eq!(clock.now(), fixed + Duration::days(7));
    }

    #[test]
    fn mock_clock_today_matches_now() {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::new(fixed);
        assert_eq!(clock.today(), NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
    }

    #[test]
    fn real_clock_returns_recent_utc() {
        let before = Utc::now();
        let now = RealClock.now();
        let after = Utc::now();
        assert!(now >= before && now <= after);
    }

    #[test]
    fn mock_clock_advance_preserves_subsecond() {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00.123456789Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::new(fixed);
        clock.advance(Duration::milliseconds(500));
        let now = clock.now();
        assert_eq!(now.timestamp_subsec_nanos(), 623_456_789);
    }
}
