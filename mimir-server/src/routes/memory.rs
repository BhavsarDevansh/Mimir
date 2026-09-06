use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::Response};
use mimir_api_types::OptimizationRunNowResponse;

use crate::error;
use crate::memory_view::{BudgetPolicy, compose_memory_view};
use crate::state::AppState;

/// Return the live condensed memory block.
///
/// Combines the LLM-condensed stable facts from system_state with a
/// freshly-rendered upcoming events section.
pub async fn memory_handler(State(state): State<Arc<AppState>>) -> Result<String, Response> {
    let view = compose_memory_view(&state).await;
    if view.core_degraded {
        return Err(error::memory_error(anyhow::anyhow!(
            view.warnings.join("; ")
        )));
    }

    Ok(view.render(BudgetPolicy::Full))
}

/// POST /memory/refresh
///
/// Triggers the memory condensation hook immediately via
/// [`HookEngine::force_run`](mimir_core::hooks::HookEngine::force_run),
/// bypassing the hook's debounce, cooldown, and idle gates.
pub async fn memory_refresh_handler(
    State(state): State<Arc<AppState>>,
) -> Result<(StatusCode, Json<OptimizationRunNowResponse>), StatusCode> {
    use mimir_core::job_queue::JobRunStatus;
    let summary = state
        .hook_engine
        .force_run("memory.condensation")
        .await
        .map_err(|e| {
            tracing::error!("Failed to trigger memory condensation: {}", e);
            match e {
                mimir_core::hooks::HookError::NotRegistered(_) => StatusCode::NOT_FOUND,
                mimir_core::hooks::HookError::AlreadyRunning(_) => StatusCode::CONFLICT,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            }
        })?;

    let status = match summary.status {
        JobRunStatus::Cancelled => StatusCode::CONFLICT,
        JobRunStatus::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::OK,
    };

    Ok((
        status,
        Json(OptimizationRunNowResponse {
            run_id: summary.run_id,
            status: summary.status.as_str().to_string(),
            started_at: summary.started_at.to_rfc3339(),
            finished_at: summary.finished_at.map(|dt| dt.to_rfc3339()),
            error: summary.error,
        }),
    ))
}
