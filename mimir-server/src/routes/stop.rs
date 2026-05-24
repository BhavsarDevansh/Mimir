use std::net::SocketAddr;
use std::sync::Arc;

use axum::{extract::ConnectInfo, extract::State, http::StatusCode};

use crate::state::AppState;

/// Trigger a graceful shutdown of the daemon.
///
/// Restricted to loopback addresses for safety.
pub async fn stop_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<StatusCode, StatusCode> {
    if !addr.ip().is_loopback() {
        return Err(StatusCode::FORBIDDEN);
    }
    let _ = state.shutdown_tx.send(true);
    Ok(StatusCode::OK)
}
