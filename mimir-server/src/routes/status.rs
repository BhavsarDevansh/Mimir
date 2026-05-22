use std::sync::Arc;
use std::time::Instant;

use axum::{Json, extract::State};

use crate::state::AppState;
use crate::types::StatusResponse;

/// Health and runtime status endpoint.
pub async fn status_handler(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let uptime = Instant::now().duration_since(state.start_time).as_secs();
    let user_depth = state.llm_client.user_queue_depth().await;
    let system_depth = state.llm_client.system_queue_depth().await;
    let workers = state.llm_client.worker_threads();

    Json(StatusResponse {
        version: mimir_core::version().to_string(),
        uptime_seconds: uptime,
        queue_depth_user: user_depth,
        queue_depth_system: system_depth,
        worker_threads: workers,
    })
}
