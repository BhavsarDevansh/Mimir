//! KB forget (soft-delete) handler.

use std::sync::Arc;

use axum::{Json, extract::State, response::Response};

use mimir_api_types::{ForgetRequest, ForgetResponse};
use mimir_knowledge::models::audit_log::ChangedBy;

use crate::error;
use crate::routes::kb::helpers::parse_datetime;
use crate::state::AppState;

pub async fn kb_forget_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForgetRequest>,
) -> Result<Json<ForgetResponse>, Response> {
    let filters = mimir_knowledge::forget::ForgetFilters {
        fact_id: body.fact_id,
        predicate: body.predicate,
        subject: body.subject,
        entity: body.entity,
        source: body.source,
        from: body.from.as_deref().and_then(parse_datetime),
        to: body.to.as_deref().and_then(parse_datetime),
        all: body.all,
    };

    let opts = mimir_knowledge::forget::ForgetOptions {
        yes: body.yes,
        confirm_sensitive: body.confirm_sensitive,
        confirmation_phrase: body.confirmation_phrase,
        archive: body.archive,
    };

    let result = state
        .knowledge_graph
        .forget_facts(filters, opts, ChangedBy::User)
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(ForgetResponse {
        forgotten_count: result.forgotten_count,
        backup_path: result.backup_path.map(|p| p.to_string_lossy().to_string()),
    }))
}
