//! Stop command handler. Sends a graceful shutdown signal to the daemon.

use std::time::Duration;

use mimir_client::MimirClient;

/// Trigger a graceful shutdown of the Mimir daemon.
///
/// After sending the stop signal, waits two seconds and then probes the
/// daemon to verify it has actually exited.
pub async fn handle_stop(base_url: &str) {
    let client = MimirClient::new(base_url);

    match client.stop().await {
        Ok(()) => {
            println!("Waiting for daemon to stop...");
            tokio::time::sleep(Duration::from_secs(2)).await;

            if crate::daemon_guard::check_daemon_reachable(base_url).await {
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
