use std::sync::Arc;

use axum::{extract::State, http::StatusCode};

use crate::state::AppState;

/// Trigger a graceful shutdown of the daemon.
pub async fn stop_handler(State(state): State<Arc<AppState>>) -> StatusCode {
    let _ = state.shutdown_tx.send(true);
    StatusCode::OK
}
