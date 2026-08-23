//! Session lifecycle: creation, deletion, listing, and existence checks.

use crate::context::ContextManager;
use crate::context::{ContextError, Session, SessionSummary};
use chrono::Utc;
use sqlx::Row;
use tracing::{debug, info};

impl ContextManager {
    pub async fn create_session(
        &self,
        system_prompt: impl Into<String>,
    ) -> Result<i64, ContextError> {
        self.insert_session(system_prompt, None).await
    }

    /// Create a session bound to an OpenAI `user` key (issue #388).
    ///
    /// The `user_key` is the conversation key clients present in the OpenAI
    /// `user` field; a fixed key resumes one ongoing conversation in the
    /// central profile.
    pub async fn create_session_with_user_key(
        &self,
        user_key: &str,
        system_prompt: impl Into<String>,
    ) -> Result<i64, ContextError> {
        self.insert_session(system_prompt, Some(user_key)).await
    }

    /// Look up the session bound to an OpenAI `user` key, if any.
    pub async fn find_session_by_user_key(
        &self,
        user_key: &str,
    ) -> Result<Option<i64>, ContextError> {
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM sessions WHERE user_key = ?1")
            .bind(user_key)
            .fetch_optional(self.pool.as_ref())
            .await?;
        if let Some(id) = id {
            self.sessions.lock().await.insert(id);
        }
        Ok(id)
    }

    /// Resolve the session for an OpenAI `user` key, creating it on first use.
    ///
    /// Race-safe: concurrent first requests for the same key may both miss
    /// the lookup, but the partial unique index on `user_key` lets exactly
    /// one insert win; the loser re-looks-up and adopts the winner's session.
    ///
    /// The system prompt is first-writer-wins and kept for the session's
    /// lifetime: the personality preset and the memory block captured at
    /// creation are stored verbatim, so later preset or memory changes apply
    /// only to sessions created afterwards (PR #466 review).
    pub async fn resolve_openai_session(
        &self,
        user_key: &str,
        system_prompt: impl Into<String>,
    ) -> Result<i64, ContextError> {
        if let Some(id) = self.find_session_by_user_key(user_key).await? {
            return Ok(id);
        }

        match self
            .create_session_with_user_key(user_key, system_prompt)
            .await
        {
            Ok(id) => Ok(id),
            Err(ContextError::Database(sqlx::Error::Database(db))) if db.is_unique_violation() => {
                self.find_session_by_user_key(user_key)
                    .await?
                    .ok_or_else(|| ContextError::Database(sqlx::Error::Database(db)))
            }
            Err(e) => Err(e),
        }
    }

    /// Insert a session row plus its system message in one transaction.
    async fn insert_session(
        &self,
        system_prompt: impl Into<String>,
        user_key: Option<&str>,
    ) -> Result<i64, ContextError> {
        let now = Utc::now();
        let prompt = system_prompt.into();

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO sessions (system_prompt, created_at, updated_at, compacted_at, user_key)
            VALUES (?1, ?2, ?2, NULL, ?3)
            "#,
        )
        .bind(&prompt)
        .bind(now)
        .bind(user_key)
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
    pub(super) async fn ensure_session_exists(&self, session_id: i64) -> Result<(), ContextError> {
        let mut sessions = self.sessions.lock().await;
        if sessions.contains(&session_id) {
            return Ok(());
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_one(self.pool.as_ref())
            .await?;

        if count == 0 {
            return Err(ContextError::SessionNotFound(session_id.to_string()));
        }

        sessions.insert(session_id);
        Ok(())
    }

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
    pub(super) async fn load_session(&self, session_id: i64) -> Result<Session, ContextError> {
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
}
