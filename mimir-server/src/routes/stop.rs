use std::sync::Arc;

use axum::{extract::State, http::StatusCode};

use crate::state::AppState;

/// Delay in milliseconds before initiating shutdown to allow HTTP response to reach the client.
const STOP_DELAY_MS: u64 = 500;

/// Trigger a graceful shutdown of the daemon.
pub async fn stop_handler(State(state): State<Arc<AppState>>) -> StatusCode {
    // Spawn shutdown with a small delay so the HTTP 200 response has time
    // to reach the client before the server stops accepting connections.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(STOP_DELAY_MS)).await;
        // Ignore errors: shutdown may already be in progress or receiver dropped.
        let _ = state.shutdown_tx.send(true);
    });
    StatusCode::OK
}
