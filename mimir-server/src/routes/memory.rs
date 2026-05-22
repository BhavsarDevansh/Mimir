use std::sync::Arc;

use axum::{extract::State, response::Response};

use crate::error;
use crate::state::AppState;

/// Return the current contents of `memory.md`.
pub async fn memory_handler(State(state): State<Arc<AppState>>) -> Result<String, Response> {
    let content = tokio::fs::read_to_string(&state.memory_path)
        .await
        .map_err(|e| error::memory_error(anyhow::Error::new(e)))?;
    Ok(content)
}
