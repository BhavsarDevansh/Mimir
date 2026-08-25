//! Entity merge-queue review handlers (issue #282): list pending suggestions,
//! apply a merge, or keep the pair separate.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};

use mimir_api_types::{EntityMergeQueueRow, MergeApplyResponse, MergeQueueListResponse};

use crate::error;
use crate::state::AppState;

/// GET /kb/merges — list pending entity-merge suggestions, newest first.
pub async fn kb_merges_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<MergeQueueListResponse>, Response> {
    let rows = state
        .knowledge_graph
        .list_entity_merges()
        .await
        .map_err(error::knowledge_error)?;

    let items: Vec<EntityMergeQueueRow> = rows
        .into_iter()
        .map(|r| EntityMergeQueueRow {
            id: r.id,
            primary_entity_id: r.primary_entity_id,
            primary_name: r.primary_name,
            primary_type: r.primary_type,
            duplicate_entity_id: r.duplicate_entity_id,
            duplicate_name: r.duplicate_name,
            duplicate_type: r.duplicate_type,
            suggested_action: r.suggested_action,
            llm_confidence: r.llm_confidence,
            queued_at: r.queued_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(MergeQueueListResponse {
        total: items.len(),
        items,
    }))
}

/// POST /kb/merges/{id}/apply — merge the duplicate entity into the primary
/// using the existing entity-merge logic.
pub async fn kb_merge_apply_handler(
    State(state): State<Arc<AppState>>,
    Path(merge_id): Path<i64>,
) -> Result<Json<MergeApplyResponse>, Response> {
    let (survivor_id, merged_id) = state
        .knowledge_graph
        .apply_entity_merge(merge_id)
        .await
        .map_err(error::knowledge_error)?;
    Ok(Json(MergeApplyResponse {
        survivor_id,
        merged_id,
    }))
}

/// POST /kb/merges/{id}/keep — mark the pair as kept separate.
pub async fn kb_merge_keep_handler(
    State(state): State<Arc<AppState>>,
    Path(merge_id): Path<i64>,
) -> Result<StatusCode, Response> {
    state
        .knowledge_graph
        .keep_entity_merge(merge_id)
        .await
        .map_err(error::knowledge_error)?;
    Ok(StatusCode::NO_CONTENT)
}
