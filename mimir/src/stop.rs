//! Stop command handler. Sends a graceful shutdown signal to the daemon.

use crate::constants::DEFAULT_BASE_URL;
use mimir_client::MimirClient;

/// Trigger a graceful shutdown of the Mimir daemon.
pub async fn handle_stop() {
    let client = MimirClient::new(DEFAULT_BASE_URL);

    match client.stop().await {
        Ok(()) => {
            println!("Mimir daemon stopped.");
        }
        Err(e) => {
            eprintln!("Failed to stop daemon: {}", e);
            std::process::exit(1);
        }
    }
}
