//! Pending-confirmation handlers: list, confirm, reject.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};

use mimir_api_types::{
    ConfirmFactResponse, PendingFactRow, PendingListResponse, RejectFactRequest,
};

use crate::error;
use crate::routes::kb::helpers::fact_row_from;
use crate::state::AppState;

pub async fn kb_pending_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PendingListResponse>, Response> {
    let rows = state
        .knowledge_graph
        .list_pending_facts()
        .await
        .map_err(error::knowledge_error)?;

    let facts: Vec<PendingFactRow> = rows
        .into_iter()
        .map(|r| PendingFactRow {
            fact_id: r.fact_id,
            subject: r.subject,
            predicate: r.predicate,
            object: r.object,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(PendingListResponse {
        total: facts.len(),
        facts,
    }))
}

/// POST /kb/facts/{id}/confirm — confirm a pending sensitive fact.
pub async fn kb_confirm_fact_handler(
    State(state): State<Arc<AppState>>,
    Path(fact_id): Path<i32>,
) -> Result<Json<ConfirmFactResponse>, Response> {
    let updated = state
        .knowledge_graph
        .confirm_fact(fact_id)
        .await
        .map_err(error::knowledge_error)?;

    let fact_row = fact_row_from(&state, &updated).await?;
    Ok(Json(ConfirmFactResponse { fact: fact_row }))
}

/// POST /kb/facts/{id}/reject — reject a pending sensitive fact (hard-delete).
pub async fn kb_reject_fact_handler(
    State(state): State<Arc<AppState>>,
    Path(fact_id): Path<i32>,
    body: Option<Json<RejectFactRequest>>,
) -> Result<StatusCode, Response> {
    // An empty body is valid; a reason, if provided, is written to the audit log.
    let reason = body.and_then(|Json(r)| r.reason);

    state
        .knowledge_graph
        .reject_fact(fact_id, reason.as_deref())
        .await
        .map_err(error::knowledge_error)?;
    Ok(StatusCode::NO_CONTENT)
}
