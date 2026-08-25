use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use mimir_api_types::{ChatMessage, SessionMessagesResponse, SessionSummary};

use crate::error;
use crate::state::AppState;

/// List all conversation sessions ordered by most recently updated first.
pub async fn sessions_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SessionSummary>>, axum::response::Response> {
    let rows = state
        .context_manager
        .list_sessions()
        .await
        .map_err(error::context_error)?;

    let result: Vec<SessionSummary> = rows
        .into_iter()
        .map(|s| SessionSummary {
            session_id: s.id,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            preview: s.preview,
            summary: s.summary,
        })
        .collect();

    Ok(Json(result))
}

/// Fetch messages for a single session from the last compaction point.
pub async fn session_messages_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<i64>,
) -> Result<Json<SessionMessagesResponse>, axum::response::Response> {
    let messages = state
        .context_manager
        .get_messages_after_compaction(session_id)
        .await
        .map_err(|e| match e {
            mimir_core::context::ContextError::SessionNotFound(_) => error::session_not_found(),
            _ => error::context_error(e),
        })?;

    let summary = state
        .context_manager
        .load_session(session_id)
        .await
        .map_err(|e| match e {
            mimir_core::context::ContextError::SessionNotFound(_) => error::session_not_found(),
            _ => error::context_error(e),
        })?
        .summary;

    let result = SessionMessagesResponse {
        session_id,
        summary,
        messages: messages
            .into_iter()
            .map(|m| ChatMessage {
                role: m.role,
                content: m.content,
                created_at: m.created_at.to_rfc3339(),
            })
            .collect(),
    };

    Ok(Json(result))
}
