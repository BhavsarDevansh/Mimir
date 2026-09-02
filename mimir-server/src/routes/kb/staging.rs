//! Unrecognized-predicate staging handlers: list, map, and reject (#468).

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};

use serde::Deserialize;

use mimir_api_types::{
    MapUnrecognizedFactRequest, MapUnrecognizedFactResponse, RejectUnrecognizedFactRequest,
    UnrecognizedFactListResponse, UnrecognizedFactRow,
};

use crate::error;
use crate::state::AppState;

const MAX_STAGED_PAGE_SIZE: i64 = 100;

#[derive(Debug, Deserialize)]
pub struct StagedListQuery {
    /// Maximum number of rows to return, bounded server-side.
    pub limit: Option<i64>,
    /// Number of rows to skip before the first returned row.
    pub offset: Option<i64>,
}

/// GET /kb/staged — list durable unrecognized-predicate facts.
pub async fn kb_staged_list_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StagedListQuery>,
) -> Result<Json<UnrecognizedFactListResponse>, Response> {
    let limit = query.limit.unwrap_or(MAX_STAGED_PAGE_SIZE);
    if !(1..=MAX_STAGED_PAGE_SIZE).contains(&limit) {
        return Err(error::bad_request(format!(
            "limit must be between 1 and {MAX_STAGED_PAGE_SIZE}"
        )));
    }
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(error::bad_request("offset must be zero or greater"));
    }
    let (rows, total) = state
        .knowledge_graph
        .list_unrecognized_facts(Some("unmapped"), limit, offset)
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
        total: usize::try_from(total).unwrap_or(usize::MAX),
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
