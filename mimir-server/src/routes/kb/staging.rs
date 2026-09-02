//! Unrecognized-predicate staging handlers: list, map, and reject (#468).

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};

use mimir_api_types::{
    MapUnrecognizedFactRequest, MapUnrecognizedFactResponse, RejectUnrecognizedFactRequest,
    UnrecognizedFactListResponse, UnrecognizedFactRow,
};

use crate::error;
use crate::state::AppState;

/// GET /kb/staged — list durable unrecognized-predicate facts.
pub async fn kb_staged_list_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UnrecognizedFactListResponse>, Response> {
    let rows = state
        .knowledge_graph
        .list_unrecognized_facts(Some("unmapped"))
        .await
        .map_err(error::knowledge_error)?;
    let items: Vec<UnrecognizedFactRow> = rows
        .into_iter()
        .map(|row| UnrecognizedFactRow {
            id: row.id,
            connector_instance_id: row.connector_instance_id,
            raw_reference: row.raw_reference,
            relationship_type_raw: row.relationship_type_raw,
            payload_json: row.payload_json,
            status: row.status,
            proposed_relationship_type_id: row.proposed_relationship_type_id,
            resolution_note: row.resolution_note,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
        })
        .collect();
    Ok(Json(UnrecognizedFactListResponse {
        total: items.len(),
        items,
    }))
}

/// POST /kb/staged/{id}/map — map a staged fact to an existing emit-eligible leaf.
pub async fn kb_staged_map_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<MapUnrecognizedFactRequest>,
) -> Result<Json<MapUnrecognizedFactResponse>, Response> {
    state
        .knowledge_graph
        .resolve_unrecognized_fact(id, request.relationship_type_id, request.note.as_deref())
        .await
        .map_err(error::knowledge_error)?;
    Ok(Json(MapUnrecognizedFactResponse {
        id,
        relationship_type_id: request.relationship_type_id,
    }))
}

/// POST /kb/staged/{id}/reject — mark a staged fact as rejected.
pub async fn kb_staged_reject_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    body: Option<Json<RejectUnrecognizedFactRequest>>,
) -> Result<StatusCode, Response> {
    let note = body.and_then(|Json(request)| request.note);
    state
        .knowledge_graph
        .reject_unrecognized_fact(id, note.as_deref())
        .await
        .map_err(error::knowledge_error)?;
    Ok(StatusCode::NO_CONTENT)
}
