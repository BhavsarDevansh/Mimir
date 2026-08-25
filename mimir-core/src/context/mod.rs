//! Multi-turn conversation context persisted in SQLite.
//!
//! Module layout by concern:
//!
//! - `core` — construction and shutdown.
//! - `sessions` — session lifecycle (create / delete / list / load).
//! - `messages` — message append, usage attribution, export, compaction reads.
//! - `trim` — context-budget trimming (turn/token caps).
//! - `compaction` — compaction primitives (candidates + commit).
//! - `compactor` — LLM summarisation orchestrator for compaction.
//! - `search` — full-text search over conversation messages.
//! - `schema` — schema initialisation and migrations.
//! - `path` — tilde-expansion helpers.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use thiserror::Error;

mod compaction;
mod compactor;
pub use compaction::CompactionCandidates;
pub use compactor::{CompactionOutcome, SessionCompactor};
mod core;
mod messages;
mod path;
mod schema;
mod search;
mod sessions;
mod trim;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[derive(Debug, Error)]
pub enum ContextError {
    /// A database operation failed.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// The requested session could not be found.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// An I/O error occurred.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A stored value could not be serialised or deserialised.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// A persisted conversation message.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ContextMessage {
    pub id: i64,
    pub session_id: i64,
    pub role: String,
    pub content: String,
    /// JSON-encoded assistant tool calls (issue #388).
    pub tool_calls: Option<String>,
    /// Tool-call id for `tool`-role messages (issue #388).
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub token_count: Option<u32>,
}

/// A persisted conversation session.
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique session identifier (auto-incrementing integer).
    pub id: i64,
    /// The system prompt that defines behaviour for this session.
    pub system_prompt: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
    /// Cumulative prompt tokens recorded across all turns.
    pub cumulative_prompt_tokens: u64,
    /// Cumulative completion tokens recorded across all turns.
    pub cumulative_completion_tokens: u64,
    /// If set, messages before this timestamp were compacted/summarised.
    pub compacted_at: Option<DateTime<Utc>>,
    /// LLM summary of the compacted turns (issue #279); injected into the
    /// exported conversation and surfaced by the session list / resume flow.
    pub summary: Option<String>,
}

/// Full conversation export for audit or logging.
#[derive(Debug, Clone)]
pub struct ConversationExport {
    pub session: Session,
    pub messages: Vec<ContextMessage>,
}

/// A lightweight summary of a conversation session for listing.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Unique session identifier (auto-incrementing integer).
    pub id: i64,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
    /// Preview of the most recent user message.
    pub preview: Option<String>,
    /// LLM summary of the compacted turns, when the session was compacted.
    pub summary: Option<String>,
}

/// Result of a full-text search over conversation messages.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageSearchResult {
    pub session_id: i64,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub snippet: String,
}

/// Manages multi-turn conversation state backed by SQLite.
#[derive(Debug, Clone)]
pub struct ContextManager {
    pool: Arc<SqlitePool>,
    sessions: Arc<Mutex<HashSet<i64>>>,
}
