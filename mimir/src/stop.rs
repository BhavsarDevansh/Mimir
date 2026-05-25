//! Stop command handler. Sends a graceful shutdown signal to the daemon.

use std::time::Duration;

use crate::constants::DEFAULT_BASE_URL;
use mimir_client::MimirClient;

/// Trigger a graceful shutdown of the Mimir daemon.
///
/// After sending the stop signal, waits two seconds and then probes the
/// daemon to verify it has actually exited.
pub async fn handle_stop() {
    let client = MimirClient::new(DEFAULT_BASE_URL);

    match client.stop().await {
        Ok(()) => {
            println!("Waiting for daemon to stop...");
            tokio::time::sleep(Duration::from_secs(2)).await;

            if crate::daemon_guard::check_daemon_reachable(DEFAULT_BASE_URL).await {
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
