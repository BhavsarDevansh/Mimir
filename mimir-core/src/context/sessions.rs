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
