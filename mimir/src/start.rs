//! Daemon launcher. Runs the Axum HTTP server in the current process.
//!
//! In the mono-binary architecture, `mimir start` is the daemon mode.
//! The server runs in the foreground; systemd (or the user) manages
//! backgrounding. No separate binary is spawned.

use mimir_core::config::Config;
use mimir_server::state::AppState;

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

    let bind_addr = config.server.bind_addr.clone();
    let state = match AppState::from_config(config).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialise server state: {}", e);
            std::process::exit(1);
        }
    };

    let app = mimir_server::build_app(std::sync::Arc::new(state));

    let addr: std::net::SocketAddr = match bind_addr.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Invalid bind address '{}': {}", bind_addr, e);
            std::process::exit(1);
        }
    };

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        }
    };

    tracing::info!("Mimir daemon listening on {}", addr);
    eprintln!("Mimir daemon listening on {}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }
}
