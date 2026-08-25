//! `GET /kb/export` handler (issue #62).

use std::sync::Arc;

use axum::{Json, extract::State, response::Response};

use mimir_api_types::{ExportFile, ExportResponse};

use crate::error;
use crate::state::AppState;

/// Serve the rendered Obsidian export bundle backing `mimir kb export`.
pub async fn kb_export_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ExportResponse>, Response> {
    let export = state
        .knowledge_graph
        .export_obsidian()
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(ExportResponse {
        files: export
            .files
            .into_iter()
            .map(|file| ExportFile {
                relative_path: file.relative_path,
                content: file.content,
            })
            .collect(),
        entity_count: export.entity_count,
        fact_count: export.fact_count,
        preference_count: export.preference_count,
        event_count: export.event_count,
    }))
}
