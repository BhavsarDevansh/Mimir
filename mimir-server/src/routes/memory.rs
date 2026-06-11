use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::Response};
use mimir_api_types::OptimizationRunNowResponse;

use crate::error;
use crate::state::AppState;

/// Return the live condensed memory block.
///
/// Combines the LLM-condensed stable facts from system_state with a
/// freshly-rendered upcoming events section.
pub async fn memory_handler(State(state): State<Arc<AppState>>) -> Result<String, Response> {
    let condensed = match state.knowledge_graph.get_condensed_memory().await {
        Ok(Some(text)) => text,
        Ok(None) => "No stable memory yet.".to_string(),
        Err(e) => {
            tracing::warn!("Failed to read condensed memory: {}", e);
            return Err(error::memory_error(anyhow::Error::new(e)));
        }
    };

    let upcoming = if let Some(user_id) = state.user_entity_id {
        match state
            .knowledge_graph
            .render_upcoming_section(user_id, 30, 10)
            .await
        {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("Failed to render upcoming section: {}", e);
                String::new()
            }
        }
    } else {
        String::new()
    };

    let combined = if upcoming.is_empty() {
        condensed
    } else {
        format!("{}\n\n{}", condensed, upcoming)
    };

    Ok(combined)
}

/// POST /memory/refresh
///
/// Triggers the memory condensation job immediately via force-submit,
/// bypassing the scheduler's debounce, cooldown, and idle gates.
pub async fn memory_refresh_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OptimizationRunNowResponse>, StatusCode> {
    use mimir_core::scheduler::DaemonJob;
    let summary = state
        .scheduler
        .force_submit(DaemonJob::MemoryCondensation)
        .await
        .map_err(|e| {
            tracing::error!("Failed to trigger memory condensation: {}", e);
            if e.is_not_registered() {
                StatusCode::NOT_FOUND
            } else if e.is_already_running() {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(Json(OptimizationRunNowResponse {
        run_id: summary.run_id,
        status: format!("{:?}", summary.status).to_lowercase(),
        started_at: summary.started_at.to_rfc3339(),
        finished_at: summary.finished_at.map(|dt| dt.to_rfc3339()),
        error: summary.error,
    }))
}
