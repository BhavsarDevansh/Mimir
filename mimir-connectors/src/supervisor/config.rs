use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorConfig {
    /// Consecutive failures before the circuit breaker trips and the
    /// connector is moved to `Error` (requires manual `resume`).
    pub max_failures: u32,
    /// Initial exponential-backoff delay applied after the first failure.
    pub base_backoff: Duration,
    /// Cap on the exponentially-growing backoff delay.
    pub max_backoff: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            max_failures: 5,
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
        }
    }
}
