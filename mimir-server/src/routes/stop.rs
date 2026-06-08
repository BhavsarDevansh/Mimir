use std::sync::Arc;

use axum::{extract::State, http::StatusCode};

use crate::state::AppState;

/// Trigger a graceful shutdown of the daemon.
pub async fn stop_handler(State(state): State<Arc<AppState>>) -> StatusCode {
    // Spawn shutdown with a small delay so the HTTP 200 response has time
    // to reach the client before the server stops accepting connections.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let _ = state.shutdown_tx.send(true);
    });
    StatusCode::OK
}
