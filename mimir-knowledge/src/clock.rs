//! Clock abstraction for testable time.

use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicI64, Ordering};

/// Provides the current UTC timestamp.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
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

    pub fn advance_seconds(&self, secs: i64) {
        self.seconds.fetch_add(secs, Ordering::SeqCst);
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
