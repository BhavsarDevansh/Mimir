use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use tracing::info;

use crate::ShutdownSource;
use crate::state::AppState;

/// Delay in milliseconds before initiating shutdown to allow HTTP response to reach the client.
const STOP_DELAY_MS: u64 = 500;

/// Trigger a graceful shutdown of the daemon.
///
/// The requesting peer is logged via [`ShutdownSource::StopEndpoint`] before
/// the shared shutdown trigger fires, so the journal records *who* requested
/// the stop (e.g. `mimir stop`) rather than only that the daemon stopped.
pub async fn stop_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> StatusCode {
    info!("{}", ShutdownSource::StopEndpoint(peer).attribution());

    // Spawn shutdown with a small delay so the HTTP 200 response has time
    // to reach the client before the server stops accepting connections.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(STOP_DELAY_MS)).await;
        // Ignore errors: shutdown may already be in progress or receiver dropped.
        let _ = state.shutdown_tx.send(true);
    });
    StatusCode::OK
}
