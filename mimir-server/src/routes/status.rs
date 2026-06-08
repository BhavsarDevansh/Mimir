use std::sync::Arc;
use std::time::Instant;

use axum::{Json, extract::State};

use mimir_api_types::StatusResponse;

use crate::state::AppState;

/// Health and runtime status endpoint.
pub async fn status_handler(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let uptime = Instant::now().duration_since(state.start_time).as_secs();
    let user_depth = state.llm_client.user_queue_depth().await;
    let system_depth = state.llm_client.system_queue_depth().await;
    let workers = state.llm_client.worker_threads();

    let (llm_reachable, context_window) = match state.llm_client.fetch_model_context_window().await
    {
        Ok(Some(window)) => (true, Some(window)),
        Ok(None) => (true, None),
        Err(_) => (false, None),
    };

    let condensed = match state.knowledge_graph.get_condensed_memory().await {
        Ok(Some(text)) => text,
        Ok(None) => String::new(),
        Err(e) => {
            tracing::warn!("Failed to read condensed memory for status: {}", e);
            String::new()
        }
    };
    let upcoming = if let Some(uid) = state.user_entity_id {
        match state
            .knowledge_graph
            .render_upcoming_section(uid, 30, 10)
            .await
        {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("Failed to render upcoming section for status: {}", e);
                String::new()
            }
        }
    } else {
        String::new()
    };

    let memory_text = if upcoming.is_empty() {
        condensed
    } else {
        format!("{}\n\n{}", condensed, upcoming)
    };

    let memory_chars = memory_text.chars().count();
    let memory_exists = !memory_text.is_empty();

    let cfg = state.config.snapshot().await;
    let memory_limit = cfg.memory.char_limit as usize;
    let memory_usage_pct = if memory_limit > 0 {
        (memory_chars as f64 / memory_limit as f64) * 100.0
    } else {
        0.0
    };

    let config_path =
        mimir_core::config::Config::config_path().map(|p| p.to_string_lossy().to_string());
    let config_exists = config_path
        .as_ref()
        .is_some_and(|p| std::path::Path::new(p).exists());

    Json(StatusResponse {
        version: mimir_core::version().to_string(),
        uptime_seconds: uptime,
        queue_depth_user: user_depth,
        queue_depth_system: system_depth,
        worker_threads: workers,
        endpoint: state.endpoint.clone(),
        model: state.model.clone(),
        config_path,
        config_exists,
        llm_reachable,
        context_window,
        memory_exists,
        memory_chars,
        memory_limit,
        memory_usage_pct,
    })
}
