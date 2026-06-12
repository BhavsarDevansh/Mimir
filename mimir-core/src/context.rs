use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use chrono::{DateTime, Utc};

use sqlx::{Acquire, Row, SqlitePool, sqlite::SqliteConnectOptions};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::fts5::escape_fts5;
use crate::llm::types::Message;

/// Errors that can occur when interacting with the context manager.
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
}

/// A persisted conversation message.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ContextMessage {
    pub id: i64,
    pub session_id: i64,
    pub role: String,
    pub content: String,
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

impl ContextManager {
    /// Create a new `ContextManager`, initialising the database schema if necessary.
    ///
    /// `db_path` may contain a leading `~` which is expanded to the user's home
    /// directory.  The parent directories are created automatically.
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, ContextError> {
        let path = expand_tilde(db_path.as_ref());

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;

        sqlx::query("PRAGMA journal_mode = WAL;")
            .execute(&pool)
            .await?;

        Self::init_schema(&pool).await?;

        #[cfg(unix)]
        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = path.parent() {
                let perms = Permissions::from_mode(0o700);
                tokio::fs::set_permissions(parent, perms).await?;
            }
        }

        info!(db_path = %path.display(), "ContextManager initialised");
        Ok(Self {
            pool: Arc::new(pool),
            sessions: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// Create a new session with the given system prompt and return its integer ID.
    pub async fn create_session(
        &self,
        system_prompt: impl Into<String>,
    ) -> Result<i64, ContextError> {
        let now = Utc::now();
        let prompt = system_prompt.into();

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO sessions (system_prompt, created_at, updated_at, compacted_at)
            VALUES (?1, ?2, ?2, NULL)
            "#,
        )
        .bind(&prompt)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
            .fetch_one(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, created_at)
            VALUES (?1, 'system', ?2, ?3)
            "#,
        )
        .bind(id)
        .bind(&prompt)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        self.sessions.lock().await.insert(id);
        debug!(session_id = %id, "created session");
        Ok(id)
    }

    /// Add a user message to the session.
    pub async fn add_user_message(
        &self,
        session_id: i64,
        content: impl Into<String>,
    ) -> Result<(), ContextError> {
        self.add_message(session_id, "user", content).await
    }

    /// Add an assistant message to the session.
    pub async fn add_assistant_message(
        &self,
        session_id: i64,
        content: impl Into<String>,
    ) -> Result<(), ContextError> {
        self.add_message(session_id, "assistant", content).await
    }

    /// Record token usage from an LLM API response.
    ///
    /// The supplied `prompt_tokens` and `completion_tokens` are treated as
    /// per-call deltas.  They are added to the stored cumulative totals and
    /// attributed to the most recent user and assistant messages respectively.
    /// Zero or negative deltas are ignored.
    pub async fn record_usage(
        &self,
        session_id: i64,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> Result<(), ContextError> {
        self.ensure_session_exists(session_id).await?;

        let prompt_delta = prompt_tokens as u64;
        let completion_delta = completion_tokens as u64;

        // Ignore non-positive deltas so we never lower stored cumulative totals.
        if completion_delta == 0 && prompt_delta == 0 {
            return Ok(());
        }

        // Attribute completion delta to the most recent assistant message.
        if completion_delta > 0 {
            sqlx::query(
                r#"
                UPDATE messages
                SET token_count = COALESCE(token_count, 0) + ?1
                WHERE id = (
                    SELECT id FROM messages
                    WHERE session_id = ?2 AND role = 'assistant'
                    ORDER BY created_at DESC
                    LIMIT 1
                )
                "#,
            )
            .bind(completion_delta as i64)
            .bind(session_id)
            .execute(self.pool.as_ref())
            .await?;
        }

        // Attribute prompt delta to the most recent user message.
        if prompt_delta > 0 {
            sqlx::query(
                r#"
                UPDATE messages
                SET token_count = COALESCE(token_count, 0) + ?1
                WHERE id = (
                    SELECT id FROM messages
                    WHERE session_id = ?2 AND role = 'user'
                    ORDER BY created_at DESC
                    LIMIT 1
                )
                "#,
            )
            .bind(prompt_delta as i64)
            .bind(session_id)
            .execute(self.pool.as_ref())
            .await?;
        }

        // Update cumulative totals on the session by adding the deltas.
        sqlx::query(
            r#"
            UPDATE sessions
            SET cumulative_prompt_tokens = cumulative_prompt_tokens + ?1,
                cumulative_completion_tokens = cumulative_completion_tokens + ?2,
                updated_at = ?3
            WHERE id = ?4
            "#,
        )
        .bind(prompt_delta as i64)
        .bind(completion_delta as i64)
        .bind(Utc::now())
        .bind(session_id)
        .execute(self.pool.as_ref())
        .await?;

        debug!(
            session_id = %session_id,
            prompt_tokens,
            completion_tokens,
            "recorded usage"
        );
        Ok(())
    }

    /// Trim the session to respect `max_turns` and an optional `max_tokens`.
    ///
    /// 1. **Turn cap (hard):** if non-system messages > `max_turns * 2`,
    ///    delete oldest complete (user, assistant) pairs.
    /// 2. **Token budget (soft):** if `max_tokens` is `Some` and cumulative
    ///    tokens exceed it, delete oldest complete pairs whose `token_count`
    ///    is known.  If unknown, fall back to halving `max_turns`.
    ///
    /// The system prompt is never removed.
    pub async fn trim_to_budget(
        &self,
        session_id: i64,
        max_tokens: Option<u32>,
        max_turns: u16,
    ) -> Result<(), ContextError> {
        self.ensure_session_exists(session_id).await?;

        // ---- Hard turn cap ----
        let non_system_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND role != 'system'",
        )
        .bind(session_id)
        .fetch_one(self.pool.as_ref())
        .await?;

        let max_messages = (max_turns as i64) * 2;
        if non_system_count > max_messages {
            let excess = non_system_count - max_messages;
            warn!(
                session_id = %session_id,
                excess,
                "trimming oldest message pairs (turn cap)"
            );
            self.delete_oldest_pairs(session_id, excess / 2 + excess % 2)
                .await?;
        }

        // ---- Soft token budget ----
        let Some(budget) = max_tokens else {
            return Ok(());
        };

        let total_tokens: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(token_count), 0) FROM messages WHERE session_id = ?1",
        )
        .bind(session_id)
        .fetch_one(self.pool.as_ref())
        .await?;

        if total_tokens > (budget as i64) {
            let unknown_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND role != 'system' AND token_count IS NULL"
            )
            .bind(session_id)
            .fetch_one(self.pool.as_ref())
            .await?;

            if unknown_count > 0 {
                // Some or all messages lack token counts: conservative fallback.
                let target_pairs = (max_turns as i64) / 2;
                let current_pairs: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) / 2 FROM messages WHERE session_id = ?1 AND role != 'system'",
                )
                .bind(session_id)
                .fetch_one(self.pool.as_ref())
                .await?;
                if current_pairs > target_pairs {
                    self.delete_oldest_pairs(session_id, current_pairs - target_pairs)
                        .await?;
                }
            } else {
                // All messages have known token counts: trim by token sum.
                let to_remove = self
                    .count_pairs_to_remove_by_tokens(session_id, budget)
                    .await?;
                if to_remove > 0 {
                    self.delete_oldest_pairs(session_id, to_remove).await?;
                }
            }
        }

        Ok(())
    }

    /// Fetch all messages for a session, ordered by creation time.
    async fn fetch_messages(&self, session_id: i64) -> Result<Vec<ContextMessage>, ContextError> {
        sqlx::query_as::<_, ContextMessage>(
            r#"
            SELECT id, session_id, role, content, created_at, token_count
            FROM messages
            WHERE session_id = ?1
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await
        .map_err(ContextError::from)
    }

    /// Export messages as OpenAI-compatible `Vec<Message>`.
    ///
    /// The earliest system message (if any) is placed first; all remaining
    /// messages are appended in chronological order.
    pub async fn export_messages(&self, session_id: i64) -> Result<Vec<Message>, ContextError> {
        self.ensure_session_exists(session_id).await?;

        let system: Vec<ContextMessage> = sqlx::query_as::<_, ContextMessage>(
            r#"
            SELECT id, session_id, role, content, created_at, token_count
            FROM messages
            WHERE session_id = ?1 AND role = 'system'
            ORDER BY created_at ASC
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let rest: Vec<ContextMessage> = sqlx::query_as::<_, ContextMessage>(
            r#"
            SELECT id, session_id, role, content, created_at, token_count
            FROM messages
            WHERE session_id = ?1 AND role != 'system'
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut result = Vec::new();
        if let Some(sys) = system.first() {
            result.push(Message {
                role: sys.role.clone(),
                content: sys.content.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
        }
        for row in rest {
            result.push(Message {
                role: row.role,
                content: row.content,
                tool_calls: None,
                tool_call_id: None,
            });
        }
        Ok(result)
    }

    /// Export the full conversation for audit or logging.
    pub async fn export_conversation(
        &self,
        session_id: i64,
    ) -> Result<ConversationExport, ContextError> {
        self.ensure_session_exists(session_id).await?;

        let session = self.load_session(session_id).await?;

        let messages = self.fetch_messages(session_id).await?;

        Ok(ConversationExport { session, messages })
    }

    /// Delete a session and all of its messages.
    pub async fn delete_session(&self, session_id: i64) -> Result<(), ContextError> {
        self.ensure_session_exists(session_id).await?;

        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(session_id)
            .execute(self.pool.as_ref())
            .await?;

        self.sessions.lock().await.remove(&session_id);
        info!(session_id = %session_id, "deleted session");
        Ok(())
    }

    /// Close the underlying database pool, flushing any pending writes.
    ///
    /// After calling `close`, any further operations will fail with a
    /// database error because the pool is no longer open.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Search conversation messages using FTS5.
    ///
    /// Results are BM25-ranked and include contextual snippets around matches.
    /// If `session_id` is provided, only messages from that session are searched.
    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<i64>,
    ) -> Result<Vec<MessageSearchResult>, ContextError> {
        let safe_query = escape_fts5(query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.min(100) as i64;

        let rows = if let Some(sid) = session_id {
            sqlx::query(
                r#"
                SELECT m.session_id, m.role, m.created_at,
                       snippet(messages_fts, -1, '<<<', '>>>', '...', 10) as snippet
                FROM messages_fts
                JOIN messages m ON m.id = messages_fts.rowid
                WHERE messages_fts MATCH ?1 AND m.session_id = ?2
                ORDER BY messages_fts.rank
                LIMIT ?3
                "#,
            )
            .bind(&safe_query)
            .bind(sid)
            .bind(limit)
            .fetch_all(self.pool.as_ref())
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT m.session_id, m.role, m.created_at,
                       snippet(messages_fts, -1, '<<<', '>>>', '...', 10) as snippet
                FROM messages_fts
                JOIN messages m ON m.id = messages_fts.rowid
                WHERE messages_fts MATCH ?1
                ORDER BY messages_fts.rank
                LIMIT ?2
                "#,
            )
            .bind(&safe_query)
            .bind(limit)
            .fetch_all(self.pool.as_ref())
            .await?
        };

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            results.push(MessageSearchResult {
                session_id: row.try_get("session_id")?,
                role: row.try_get("role")?,
                created_at: row.try_get("created_at")?,
                snippet: row.try_get("snippet")?,
            });
        }
        Ok(results)
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    async fn init_schema(pool: &SqlitePool) -> Result<(), ContextError> {
        // Detect legacy TEXT session IDs and migrate if necessary.
        let session_id_type: Option<String> =
            sqlx::query_scalar("SELECT type FROM pragma_table_info('sessions') WHERE name = 'id'")
                .fetch_optional(pool)
                .await?;

        if session_id_type.as_deref() == Some("TEXT") {
            migrate_text_to_integer(pool).await?;
        }

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                system_prompt TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
                cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
                summary TEXT,
                compacted_at TEXT
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                token_count INTEGER
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_messages_session
            ON messages(session_id, created_at)
            "#,
        )
        .execute(pool)
        .await?;

        // Migration: add compacted_at to existing databases where it is missing.
        let has_compacted_at: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'compacted_at'",
        )
        .fetch_one(pool)
        .await?;

        if has_compacted_at == 0 {
            sqlx::query("ALTER TABLE sessions ADD COLUMN compacted_at TEXT")
                .execute(pool)
                .await?;
        }

        // FTS5 virtual table for full-text search over messages.
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                role,
                content,
                content='messages',
                content_rowid='id'
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, role, content)
                VALUES (new.id, new.role, new.content);
            END;
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, role, content)
                VALUES ('delete', old.id, old.role, old.content);
            END;
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, role, content)
                VALUES ('delete', old.id, old.role, old.content);
                INSERT INTO messages_fts(rowid, role, content)
                VALUES (new.id, new.role, new.content);
            END;
            "#,
        )
        .execute(pool)
        .await?;

        // Rebuild FTS5 index to ensure it is in sync with messages table.
        sqlx::query("INSERT INTO messages_fts(messages_fts) VALUES('rebuild');")
            .execute(pool)
            .await?;

        Ok(())
    }

    async fn add_message(
        &self,
        session_id: i64,
        role: &str,
        content: impl Into<String>,
    ) -> Result<(), ContextError> {
        self.ensure_session_exists(session_id).await?;

        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .bind(session_id)
        .bind(role)
        .bind(content.into())
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;

        sqlx::query("UPDATE sessions SET updated_at = ?1 WHERE id = ?2")
            .bind(now)
            .bind(session_id)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    async fn ensure_session_exists(&self, session_id: i64) -> Result<(), ContextError> {
        if self.sessions.lock().await.contains(&session_id) {
            return Ok(());
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_one(self.pool.as_ref())
            .await?;

        if count == 0 {
            return Err(ContextError::SessionNotFound(session_id.to_string()));
        }

        self.sessions.lock().await.insert(session_id);
        Ok(())
    }

    /// List all sessions ordered by most recently updated first.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ContextError> {
        let rows = sqlx::query(
            r#"
            SELECT
                s.id,
                s.created_at,
                s.updated_at,
                (
                    SELECT content FROM messages
                    WHERE session_id = s.id AND role = 'user'
                    ORDER BY created_at DESC
                    LIMIT 1
                ) as preview
            FROM sessions s
            ORDER BY s.updated_at DESC
            "#,
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut result = Vec::new();
        for row in rows {
            result.push(SessionSummary {
                id: row.try_get("id")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                preview: row.try_get("preview").ok(),
            });
        }
        Ok(result)
    }

    /// Return all messages for a session from the last compaction point (or all
    /// messages if the session has never been compacted).
    pub async fn get_messages_after_compaction(
        &self,
        session_id: i64,
    ) -> Result<Vec<ContextMessage>, ContextError> {
        self.ensure_session_exists(session_id).await?;

        let compacted_at: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT compacted_at FROM sessions WHERE id = ?1")
                .bind(session_id)
                .fetch_one(self.pool.as_ref())
                .await?;

        let messages = if let Some(ts) = compacted_at {
            sqlx::query_as::<_, ContextMessage>(
                r#"
                SELECT id, session_id, role, content, created_at, token_count
                FROM messages
                WHERE session_id = ?1 AND created_at >= ?2
                ORDER BY created_at ASC
                "#,
            )
            .bind(session_id)
            .bind(ts)
            .fetch_all(self.pool.as_ref())
            .await?
        } else {
            self.fetch_messages(session_id).await?
        };

        Ok(messages)
    }

    async fn load_session(&self, session_id: i64) -> Result<Session, ContextError> {
        let row = sqlx::query(
            r#"
            SELECT id, system_prompt, created_at, updated_at,
                   cumulative_prompt_tokens, cumulative_completion_tokens,
                   compacted_at
            FROM sessions
            WHERE id = ?1
            "#,
        )
        .bind(session_id)
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(Session {
            id: row.try_get("id")?,
            system_prompt: row.try_get("system_prompt")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            cumulative_prompt_tokens: row.try_get::<i64, _>("cumulative_prompt_tokens")? as u64,
            cumulative_completion_tokens: row.try_get::<i64, _>("cumulative_completion_tokens")?
                as u64,
            compacted_at: row.try_get("compacted_at").ok(),
        })
    }

    /// Count how many complete (user, assistant) pairs must be removed so that
    /// the remaining messages' total token count is <= `max_tokens`.
    async fn count_pairs_to_remove_by_tokens(
        &self,
        session_id: i64,
        max_tokens: u32,
    ) -> Result<i64, ContextError> {
        let rows: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
            r#"
            SELECT id, role, token_count FROM messages
            WHERE session_id = ?1 AND role != 'system'
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let total: i64 = rows.iter().filter_map(|(_, _, t)| *t).sum();
        if total <= (max_tokens as i64) {
            return Ok(0);
        }

        let mut running = 0i64;
        let mut pairs = 0i64;
        let mut pending_user: Option<Option<i64>> = None;

        for (_id, role, tokens) in rows {
            if role == "user" {
                pending_user = Some(tokens);
            } else if role == "assistant" && pending_user.is_some() {
                let user_tokens = pending_user.take().unwrap_or(Some(0)).unwrap_or(0);
                let assistant_tokens = tokens.unwrap_or(0);
                let pair_sum = user_tokens + assistant_tokens;
                running += pair_sum;
                pairs += 1;
                if total - running <= (max_tokens as i64) {
                    return Ok(pairs);
                }
            }
            // Skip orphaned assistants (no preceding user) and unmatched users.
        }

        Ok(pairs)
    }

    /// Delete the oldest `n` complete (user, assistant) pairs from the session.
    async fn delete_oldest_pairs(&self, session_id: i64, n: i64) -> Result<(), ContextError> {
        if n <= 0 {
            return Ok(());
        }

        let rows: Vec<(i64, String)> = sqlx::query_as(
            r#"
            SELECT id, role FROM messages
            WHERE session_id = ?1 AND role != 'system'
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut ids_to_delete = Vec::new();
        let mut pending_user: Option<i64> = None;
        let mut pairs_found = 0i64;

        for (id, role) in rows {
            if pairs_found >= n {
                break;
            }
            if role == "user" {
                pending_user = Some(id);
            } else if role == "assistant" && pending_user.is_some() {
                ids_to_delete.push(pending_user.take().unwrap());
                ids_to_delete.push(id);
                pairs_found += 1;
            }
            // Skip orphaned assistants (no preceding user).
        }

        for id in ids_to_delete {
            sqlx::query("DELETE FROM messages WHERE id = ?1")
                .bind(id)
                .execute(self.pool.as_ref())
                .await?;
        }

        debug!(session_id = %session_id, pairs_removed = pairs_found, "deleted oldest pairs");
        Ok(())
    }
}

async fn migrate_text_to_integer(pool: &SqlitePool) -> Result<(), ContextError> {
    let mut conn = pool.acquire().await?;

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await?;

    let mut tx = conn.begin().await?;

    // Create new sessions table with INTEGER PRIMARY KEY.
    sqlx::query(
        r#"
        CREATE TABLE sessions_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            system_prompt TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
            cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
            summary TEXT,
            compacted_at TEXT
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    // Migrate sessions in created_at order, capturing new integer IDs.
    #[derive(sqlx::FromRow)]
    struct OldSession {
        id: String,
        system_prompt: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        cumulative_prompt_tokens: i64,
        cumulative_completion_tokens: i64,
        summary: Option<String>,
        compacted_at: Option<DateTime<Utc>>,
    }

    let old_sessions: Vec<OldSession> =
        sqlx::query_as("SELECT * FROM sessions ORDER BY created_at")
            .fetch_all(&mut *tx)
            .await?;

    let mut mapping = Vec::with_capacity(old_sessions.len());
    for old in old_sessions {
        sqlx::query(
            r#"
            INSERT INTO sessions_new (system_prompt, created_at, updated_at,
                cumulative_prompt_tokens, cumulative_completion_tokens, summary, compacted_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&old.system_prompt)
        .bind(old.created_at)
        .bind(old.updated_at)
        .bind(old.cumulative_prompt_tokens)
        .bind(old.cumulative_completion_tokens)
        .bind(&old.summary)
        .bind(old.compacted_at)
        .execute(&mut *tx)
        .await?;

        let new_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
            .fetch_one(&mut *tx)
            .await?;
        mapping.push((old.id, new_id));
    }

    // Create new messages table with INTEGER session_id.
    sqlx::query(
        r#"
        CREATE TABLE messages_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL REFERENCES sessions_new(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL,
            token_count INTEGER
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    // Migrate messages preserving original id values.
    for (old_sid, new_sid) in &mapping {
        sqlx::query(
            r#"
            INSERT INTO messages_new (id, session_id, role, content, created_at, token_count)
            SELECT id, ?1, role, content, created_at, token_count
            FROM messages WHERE session_id = ?2
            "#,
        )
        .bind(new_sid)
        .bind(old_sid)
        .execute(&mut *tx)
        .await?;
    }

    // Drop old tables.
    sqlx::query("DROP TABLE messages").execute(&mut *tx).await?;
    sqlx::query("DROP TABLE sessions").execute(&mut *tx).await?;

    // Rename new tables.
    sqlx::query("ALTER TABLE sessions_new RENAME TO sessions")
        .execute(&mut *tx)
        .await?;
    sqlx::query("ALTER TABLE messages_new RENAME TO messages")
        .execute(&mut *tx)
        .await?;

    // Recreate index.
    sqlx::query(
        r#"
        CREATE INDEX idx_messages_session ON messages(session_id, created_at)
        "#,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await?;

    info!("Migrated sessions.id from TEXT to INTEGER PRIMARY KEY");
    Ok(())
}

fn expand_tilde(path: &Path) -> PathBuf {
    if let Some(s) = path.to_str()
        && let Some(stripped) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    path.to_path_buf()
}

#[cfg(test)]
fn expand_tilde_with_home(path: &Path, home: &Path) -> PathBuf {
    if let Some(s) = path.to_str()
        && let Some(stripped) = s.strip_prefix("~/")
    {
        return home.join(stripped);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_manager() -> (ContextManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let mgr = ContextManager::new(&db).await.unwrap();
        (mgr, dir)
    }

    #[tokio::test]
    async fn create_session_returns_i64() {
        let (mgr, _dir) = setup_manager().await;
        let id = mgr
            .create_session("You are a test assistant")
            .await
            .unwrap();
        assert!(id > 0, "expected positive i64 session id, got {id}");
    }

    #[tokio::test]
    async fn add_user_and_assistant_messages_persist() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();

        mgr.add_user_message(sid, "hello").await.unwrap();
        mgr.add_assistant_message(sid, "hi there").await.unwrap();

        let msgs = mgr.export_messages(sid).await.unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
    }

    #[tokio::test]
    async fn trim_respects_max_turns() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();

        for i in 0..25 {
            mgr.add_user_message(sid, format!("msg {}", i))
                .await
                .unwrap();
            mgr.add_assistant_message(sid, format!("reply {}", i))
                .await
                .unwrap();
        }

        mgr.trim_to_budget(sid, Some(4096), 20).await.unwrap();

        let msgs = mgr.export_messages(sid).await.unwrap();
        assert_eq!(msgs.len(), 41);
        assert_eq!(msgs[0].role, "system");
    }

    #[tokio::test]
    async fn trim_respects_max_tokens_after_usage_recorded() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();

        for i in 0..10 {
            mgr.add_user_message(sid, format!("u{}", i)).await.unwrap();
            mgr.add_assistant_message(sid, format!("a{}", i))
                .await
                .unwrap();
            mgr.record_usage(sid, ((i + 1) * 500) as u32, ((i + 1) * 500) as u32)
                .await
                .unwrap();
        }

        mgr.trim_to_budget(sid, Some(2000), 100).await.unwrap();

        let msgs = mgr.export_messages(sid).await.unwrap();
        assert!(
            msgs.len() <= 9,
            "expected at most 9 messages, got {}",
            msgs.len()
        );
        assert_eq!(msgs[0].role, "system");
    }

    #[tokio::test]
    async fn system_prompt_never_trimmed() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("precious system prompt").await.unwrap();

        for i in 0..5 {
            mgr.add_user_message(sid, format!("u{}", i)).await.unwrap();
            mgr.add_assistant_message(sid, format!("a{}", i))
                .await
                .unwrap();
        }

        mgr.trim_to_budget(sid, Some(1), 1).await.unwrap();
        let msgs = mgr.export_messages(sid).await.unwrap();
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, "precious system prompt");
    }

    #[tokio::test]
    async fn export_messages_orders_system_first() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "u1").await.unwrap();
        mgr.add_assistant_message(sid, "a1").await.unwrap();

        let msgs = mgr.export_messages(sid).await.unwrap();
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
    }

    #[tokio::test]
    async fn record_usage_attribution() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();

        mgr.add_user_message(sid, "hello").await.unwrap();
        mgr.add_assistant_message(sid, "world").await.unwrap();
        mgr.record_usage(sid, 10, 5).await.unwrap();

        let conv = mgr.export_conversation(sid).await.unwrap();
        assert_eq!(conv.session.cumulative_prompt_tokens, 10);
        assert_eq!(conv.session.cumulative_completion_tokens, 5);

        mgr.add_user_message(sid, "how?").await.unwrap();
        mgr.add_assistant_message(sid, "fine").await.unwrap();
        // Pass deltas, not cumulative totals.
        mgr.record_usage(sid, 15, 7).await.unwrap();

        let conv2 = mgr.export_conversation(sid).await.unwrap();
        assert_eq!(conv2.session.cumulative_prompt_tokens, 25);
        assert_eq!(conv2.session.cumulative_completion_tokens, 12);

        let rows: Vec<ContextMessage> = sqlx::query_as::<_, ContextMessage>(
            "SELECT * FROM messages WHERE session_id = ?1 AND role = 'user' ORDER BY created_at ASC"
        )
        .bind(sid)
        .fetch_all(mgr.pool.as_ref())
        .await
        .unwrap();

        assert_eq!(rows.len(), 2);
        // First user message got 10 prompt tokens.
        assert_eq!(rows[0].token_count, Some(10));
        // Second user message got 15 prompt tokens (delta).
        assert_eq!(rows[1].token_count, Some(15));
    }

    #[tokio::test]
    async fn unknown_session_returns_error() {
        let (mgr, _dir) = setup_manager().await;
        let result = mgr.add_user_message(999_999, "x").await;
        assert!(matches!(result, Err(ContextError::SessionNotFound(_))));
    }

    #[tokio::test]
    async fn delete_session_cascade_removes_messages() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "u1").await.unwrap();
        mgr.delete_session(sid).await.unwrap();

        let result = mgr.export_messages(sid).await;
        assert!(matches!(result, Err(ContextError::SessionNotFound(_))));
    }

    #[tokio::test]
    async fn reload_from_db_restores_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("persist.db");

        {
            let mgr = ContextManager::new(&db).await.unwrap();
            let sid = mgr.create_session("sys").await.unwrap();
            mgr.add_user_message(sid, "hello").await.unwrap();
            mgr.add_assistant_message(sid, "world").await.unwrap();
        }

        let mgr2 = ContextManager::new(&db).await.unwrap();
        let sids: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions")
            .fetch_all(mgr2.pool.as_ref())
            .await
            .unwrap();
        assert_eq!(sids.len(), 1);

        let msgs = mgr2.export_messages(sids[0]).await.unwrap();
        assert_eq!(msgs.len(), 3);
    }

    #[tokio::test]
    async fn db_path_with_tilde_expanded() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        // Test tilde expansion directly without mutating process env.
        let expanded = expand_tilde_with_home(
            std::path::Path::new("~/nonexistent_test_mimir_ctx.db"),
            home,
        );
        assert_eq!(expanded, home.join("nonexistent_test_mimir_ctx.db"));

        let mgr = ContextManager::new(expanded.to_str().unwrap()).await;
        assert!(
            mgr.is_ok(),
            "ContextManager should succeed with expanded path"
        );
        assert!(
            expanded.exists(),
            "DB file should be created under temp HOME"
        );

        // Clean up session if created (best-effort).
        if let Ok(ref m) = mgr {
            let sessions = m.list_sessions().await.unwrap();
            for s in sessions {
                let _ = m.delete_session(s.id).await;
            }
        }
    }

    #[tokio::test]
    async fn list_sessions_orders_by_updated_at_desc() {
        let (mgr, _dir) = setup_manager().await;
        let sid1 = mgr.create_session("sys1").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let sid2 = mgr.create_session("sys2").await.unwrap();

        mgr.add_user_message(sid1, "first").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        mgr.add_user_message(sid2, "second").await.unwrap();

        let list = mgr.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, sid2);
        assert_eq!(list[1].id, sid1);
    }

    #[tokio::test]
    async fn list_sessions_preview_is_latest_user_message() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "hello").await.unwrap();
        mgr.add_assistant_message(sid, "hi").await.unwrap();
        mgr.add_user_message(sid, "world").await.unwrap();

        let list = mgr.list_sessions().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].preview, Some("world".to_string()));
    }

    #[tokio::test]
    async fn list_sessions_empty_db_returns_empty() {
        let (mgr, _dir) = setup_manager().await;
        let list = mgr.list_sessions().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn get_messages_after_compaction_returns_all_when_null() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "u1").await.unwrap();
        mgr.add_assistant_message(sid, "a1").await.unwrap();

        let msgs = mgr.get_messages_after_compaction(sid).await.unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
    }

    #[tokio::test]
    async fn get_messages_after_compaction_returns_only_after_timestamp() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "old").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mid = Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        mgr.add_user_message(sid, "new").await.unwrap();
        mgr.add_assistant_message(sid, "reply").await.unwrap();

        sqlx::query("UPDATE sessions SET compacted_at = ?1 WHERE id = ?2")
            .bind(mid)
            .bind(sid)
            .execute(mgr.pool.as_ref())
            .await
            .unwrap();

        let msgs = mgr.get_messages_after_compaction(sid).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "new");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[tokio::test]
    async fn get_messages_after_compaction_unknown_session_errors() {
        let (mgr, _dir) = setup_manager().await;
        let result = mgr.get_messages_after_compaction(999_999).await;
        assert!(matches!(result, Err(ContextError::SessionNotFound(_))));
    }

    #[tokio::test]
    async fn schema_migration_adds_compacted_at() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("migrate.db");

        // Create an old-style database without compacted_at.
        {
            let pool = sqlx::SqlitePool::connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&db)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
            sqlx::query(
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    system_prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
                    summary TEXT
                )
                "#,
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                r#"
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    token_count INTEGER
                )
                "#,
            )
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
        }

        // ContextManager::new should migrate it.
        let mgr = ContextManager::new(&db).await.unwrap();
        let sid = mgr.create_session("sys").await.unwrap();
        let conv = mgr.export_conversation(sid).await.unwrap();
        assert!(conv.session.compacted_at.is_none());
    }

    #[tokio::test]
    async fn schema_migration_text_to_integer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("migrate_text.db");

        // Create an old-style database with TEXT session IDs.
        {
            let pool = sqlx::SqlitePool::connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&db)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
            sqlx::query(
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    system_prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
                    summary TEXT,
                    compacted_at TEXT
                )
                "#,
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                r#"
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    token_count INTEGER
                )
                "#,
            )
            .execute(&pool)
            .await
            .unwrap();
            // Seed data with TEXT session IDs.
            sqlx::query(
                "INSERT INTO sessions (id, system_prompt, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)"
            )
            .bind("old-session-uuid")
            .bind("old sys")
            .bind(Utc::now())
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO messages (session_id, role, content, created_at) VALUES (?1, 'user', 'hello world', ?2)"
            )
            .bind("old-session-uuid")
            .bind(Utc::now())
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
        }

        let mgr = ContextManager::new(&db).await.unwrap();
        let sid = mgr.create_session("sys").await.unwrap();
        assert!(sid > 0);

        // Verify old data was migrated.
        let sessions = mgr.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2); // old + new
        let old_session = sessions.iter().find(|s| s.id != sid).unwrap();
        let msgs = mgr.export_messages(old_session.id).await.unwrap();
        assert_eq!(msgs.len(), 1); // only user message (old session had no system message)
        assert!(msgs.iter().any(|m| m.content == "hello world"));

        // Verify search works on migrated data.
        let results = mgr.search_messages("hello", 10, None).await.unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.snippet.contains("hello")));
    }

    #[tokio::test]
    async fn search_messages_basic() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "the quick brown fox")
            .await
            .unwrap();
        mgr.add_assistant_message(sid, "jumps over the lazy dog")
            .await
            .unwrap();

        let results = mgr.search_messages("fox", 10, None).await.unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.snippet.contains("<<<fox>>>")));
    }

    #[tokio::test]
    async fn search_messages_session_filter() {
        let (mgr, _dir) = setup_manager().await;
        let sid1 = mgr.create_session("sys1").await.unwrap();
        let sid2 = mgr.create_session("sys2").await.unwrap();

        mgr.add_user_message(sid1, "unique keyword alpha")
            .await
            .unwrap();
        mgr.add_user_message(sid2, "unique keyword beta")
            .await
            .unwrap();

        let results = mgr.search_messages("alpha", 10, Some(sid1)).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, sid1);
    }

    #[tokio::test]
    async fn search_messages_no_results() {
        let (mgr, _dir) = setup_manager().await;
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "hello world").await.unwrap();

        let results = mgr
            .search_messages("xyznonsense123", 10, None)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_context_manager_close() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("close_test.db");
        let mgr = ContextManager::new(&db).await.unwrap();
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "hello").await.unwrap();

        mgr.close().await;

        // After close, any operation should fail because the pool is closed.
        let result = mgr.add_user_message(sid, "world").await;
        assert!(
            matches!(result, Err(ContextError::Database(_))),
            "expected database error after close, got: {:?}",
            result
        );
    }
}
