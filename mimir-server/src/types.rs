use mimir_core::llm::types::Usage;
use serde::{Deserialize, Serialize};

/// Request body for chat endpoints.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// Existing session id; if omitted a new session is created.
    pub session_id: Option<String>,
    /// User message content.
    pub message: String,
}

/// Response body for the non-streaming chat endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatResponse {
    pub session_id: String,
    pub response: String,
    pub usage: Usage,
}

/// Response body for the status endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub version: String,
    pub uptime_seconds: u64,
    pub queue_depth_user: usize,
    pub queue_depth_system: usize,
    pub worker_threads: u8,
}
