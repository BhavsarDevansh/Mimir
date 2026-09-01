//! Stop command handler. Sends a graceful shutdown signal to the daemon.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Trigger a graceful shutdown of the Mimir daemon.
///
/// After sending the stop signal, polls until the daemon is unreachable or
/// the stop timeout expires.
pub async fn handle_stop(transport: &crate::transport::DaemonTransport) {
    let client = crate::cli_util::make_client(transport);

    match client.stop().await {
        Ok(()) => {
            println!("Waiting for daemon to stop...");
            if !wait_until_stopped(&TransportProbe, transport, STOP_POLL_INTERVAL, STOP_TIMEOUT)
                .await
            {
                eprintln!("Warning: daemon is still reachable after stop signal.");
                std::process::exit(1);
            }

            println!("Mimir daemon stopped.");
        }
        Err(e) => {
            eprintln!("Failed to stop daemon: {}", e);
            std::process::exit(1);
        }
    }
}

trait ReachabilityProbe: Send + Sync {
    fn check<'a>(
        &'a self,
        transport: &'a crate::transport::DaemonTransport,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}

struct TransportProbe;

impl ReachabilityProbe for TransportProbe {
    fn check<'a>(
        &'a self,
        transport: &'a crate::transport::DaemonTransport,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(crate::daemon_guard::check_daemon_reachable(transport))
    }
}

async fn wait_until_stopped(
    probe: &impl ReachabilityProbe,
    transport: &crate::transport::DaemonTransport,
    poll_interval: Duration,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if !probe.check(transport).await {
            return true;
        }

        tokio::time::sleep(poll_interval).await;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::DaemonTransport;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct SequenceProbe {
        reachable: Mutex<VecDeque<bool>>,
        calls: Mutex<usize>,
    }

    impl SequenceProbe {
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl ReachabilityProbe for SequenceProbe {
        fn check<'a>(
            &'a self,
            _transport: &'a DaemonTransport,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async move {
                *self.calls.lock().unwrap() += 1;
                let mut reachable = self.reachable.lock().unwrap();
                reachable.pop_front().unwrap()
            })
        }
    }

    #[tokio::test]
    async fn wait_until_stopped_polls_without_fixed_wait() {
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let probe = SequenceProbe {
            reachable: Mutex::new([true, false].into_iter().collect()),
            calls: Mutex::new(0),
        };
        let started = Instant::now();

        let stopped = wait_until_stopped(
            &probe,
            &transport,
            Duration::from_millis(1),
            Duration::from_secs(5),
        )
        .await;

        assert!(stopped);
        assert_eq!(probe.calls(), 2);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn wait_until_stopped_times_out_when_still_reachable() {
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let probe = SequenceProbe {
            reachable: Mutex::new([true, true, true].into_iter().collect()),
            calls: Mutex::new(0),
        };

        let stopped = wait_until_stopped(
            &probe,
            &transport,
            Duration::from_millis(1),
            Duration::from_millis(5),
        )
        .await;

        assert!(!stopped);
        assert_eq!(probe.calls(), 3);
    }
}
