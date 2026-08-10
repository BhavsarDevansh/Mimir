//! KB trash handlers: list, restore, empty.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::Response,
};

use mimir_api_types::{RestoreRequest, RestoreResponse, TrashListResponse, TrashRow};
use mimir_knowledge::models::audit_log::ChangedBy;

use crate::error;
use crate::routes::kb::params::TrashQueryParams;
use crate::state::AppState;

pub async fn kb_trash_list_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TrashQueryParams>,
) -> Result<Json<TrashListResponse>, Response> {
    let items = state
        .knowledge_graph
        .list_trash(params.limit as i64, params.offset as i64)
        .await
        .map_err(error::knowledge_error)?;

    let total = items.len() as i64; // approximate
    let rows: Vec<TrashRow> = items
        .into_iter()
        .map(|i| TrashRow {
            trash_id: i.trash_id,
            subject: i.subject_name,
            predicate: i.relationship_type_name,
            object: i.object_name.or(i.object_literal),
            deleted_at: i.deleted_at.to_rfc3339(),
            expires_at: i.expires_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(TrashListResponse {
        total,
        offset: params.offset,
        limit: params.limit,
        items: rows,
    }))
}

pub async fn kb_trash_restore_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    if body.all {
        let facts = state
            .knowledge_graph
            .restore_all(ChangedBy::User)
            .await
            .map_err(error::knowledge_error)?;
        Ok(Json(RestoreResponse {
            restored_count: facts.len(),
        }))
    } else {
        let id = body.trash_id.ok_or_else(error::json_rejection)?;
        let _fact = state
            .knowledge_graph
            .restore_fact(id, ChangedBy::User)
            .await
            .map_err(error::knowledge_error)?;
        Ok(Json(RestoreResponse { restored_count: 1 }))
    }
}

pub async fn kb_trash_empty_handler(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, Response> {
    state
        .knowledge_graph
        .empty_trash()
        .await
        .map_err(error::knowledge_error)?;
    Ok(StatusCode::NO_CONTENT)
}
