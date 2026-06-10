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
    total_nanos: AtomicI64,
}

impl MockClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            total_nanos: AtomicI64::new(start.timestamp_nanos_opt().expect("valid timestamp")),
        }
    }

    pub fn advance(&self, duration: Duration) {
        let delta = duration.num_nanoseconds().unwrap_or(0);
        self.total_nanos.fetch_add(delta, Ordering::SeqCst);
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        let nanos = self.total_nanos.load(Ordering::SeqCst);
        DateTime::from_timestamp_nanos(nanos)
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
    fn mock_clock_advance_negative_duration() {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::new(fixed);
        clock.advance(Duration::days(-3));
        assert_eq!(clock.now(), fixed - Duration::days(3));
    }

    #[test]
    fn mock_clock_advance_negative_subsecond() {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00.500Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = MockClock::new(fixed);
        clock.advance(Duration::milliseconds(-200));
        assert_eq!(clock.now(), fixed - Duration::milliseconds(200));
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
