//! Knowledge-base optimization handlers.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};

use mimir_api_types::{
    OptimizationRunNowResponse, OptimizationRunSummary, OptimizationStatusResponse,
};

use crate::state::AppState;

pub async fn kb_optimization_status_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OptimizationStatusResponse>, StatusCode> {
    let status = state
        .job_queue
        .status("knowledge.optimization")
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch optimization status: {}", e);
            if e.is_not_registered() {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(Json(OptimizationStatusResponse {
        job_id: status.job_id,
        priority: format!("{:?}", status.priority).to_lowercase(),
        schedule: status.schedule.map(|s| s.as_hhmm()),
        next_run_at: status.next_run_at.map(|dt| dt.to_rfc3339()),
        last_run: status.last_run.map(|run| OptimizationRunSummary {
            run_id: run.run_id,
            status: format!("{:?}", run.status).to_lowercase(),
            started_at: run.started_at.to_rfc3339(),
            finished_at: run.finished_at.map(|dt| dt.to_rfc3339()),
            error: run.error,
        }),
    }))
}

/// POST /kb/optimization/run-now
pub async fn kb_optimization_run_now_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OptimizationRunNowResponse>, StatusCode> {
    let summary = state
        .job_queue
        .run_now("knowledge.optimization")
        .await
        .map_err(|e| {
            tracing::error!("Failed to run optimization: {}", e);
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
