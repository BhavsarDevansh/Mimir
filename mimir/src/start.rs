//! Daemon launcher. Runs the Axum HTTP server in the current process.
//!
//! In the mono-binary architecture, `mimir start` is the daemon mode.
//! The server runs in the foreground; systemd (or the user) manages
//! backgrounding. No separate binary is spawned.

use std::sync::Arc;

use mimir_core::config::{Config, ReloadableConfig};

/// Start the Mimir HTTP server in the foreground.
///
/// Loads config, initialises shared state, binds to the configured address,
/// and runs until the process is terminated (SIGINT, SIGTERM, or `mimir stop`).
pub async fn handle_start() {
    tracing_subscriber::fmt::init();

    let config = match Config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    let config_path = Config::config_path().unwrap_or_else(|| {
        eprintln!("Failed to resolve config path");
        std::process::exit(1);
    });

    let reloadable = Arc::new(ReloadableConfig::new(config, config_path));

    if let Err(e) = mimir_server::start_server(reloadable).await {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }
}
