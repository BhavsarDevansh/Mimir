//! Conversation messages: append, usage attribution, export, and compaction reads.

use crate::context::ContextManager;
use crate::context::{ContextError, ContextMessage, ConversationExport};
use crate::llm::types::Message;
use chrono::{DateTime, Utc};
use tracing::debug;

impl ContextManager {
    pub async fn add_user_message(
        &self,
        session_id: i64,
        content: impl Into<String>,
    ) -> Result<(), ContextError> {
        self.add_message(session_id, "user", content).await
    }

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

    /// Fetch every persisted message for a session in chronological order.
    async fn fetch_messages(&self, session_id: i64) -> Result<Vec<ContextMessage>, ContextError> {
        sqlx::query_as::<_, ContextMessage>(
            r#"
            SELECT id, session_id, role, content, tool_calls, tool_call_id, created_at, token_count
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
    pub async fn export_messages(&self, session_id: i64) -> Result<Vec<Message>, ContextError> {
        self.ensure_session_exists(session_id).await?;

        let system: Vec<ContextMessage> = sqlx::query_as::<_, ContextMessage>(
            r#"
            SELECT id, session_id, role, content, tool_calls, tool_call_id, created_at, token_count
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
            SELECT id, session_id, role, content, tool_calls, tool_call_id, created_at, token_count
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
            let tool_calls = match row.tool_calls {
                Some(json) => Some(serde_json::from_str(&json)?),
                None => None,
            };
            result.push(Message {
                role: row.role,
                content: row.content,
                tool_calls,
                tool_call_id: row.tool_call_id,
            });
        }
        Ok(result)
    }

    pub async fn export_conversation(
        &self,
        session_id: i64,
    ) -> Result<ConversationExport, ContextError> {
        self.ensure_session_exists(session_id).await?;

        let session = self.load_session(session_id).await?;

        let messages = self.fetch_messages(session_id).await?;

        Ok(ConversationExport { session, messages })
    }

    /// Persist a `tool`-role message carrying a tool result (issue #388).
    pub async fn add_tool_message(
        &self,
        session_id: i64,
        tool_call_id: &str,
        content: impl Into<String>,
    ) -> Result<(), ContextError> {
        self.add_message_with_tool(session_id, "tool", content, None, Some(tool_call_id))
            .await
    }

    /// Persist an assistant message that issued tool calls (issue #388).
    ///
    /// The tool calls are stored as JSON so `export_messages` can round-trip
    /// them into the OpenAI-shaped conversation for the next LLM call.
    pub async fn add_assistant_tool_calls_message(
        &self,
        session_id: i64,
        content: impl Into<String>,
        tool_calls: &[crate::llm::types::ToolCall],
    ) -> Result<(), ContextError> {
        let json = serde_json::to_string(tool_calls)?;
        self.add_message_with_tool(session_id, "assistant", content, Some(&json), None)
            .await
    }

    async fn add_message(
        &self,
        session_id: i64,
        role: &str,
        content: impl Into<String>,
    ) -> Result<(), ContextError> {
        self.add_message_with_tool(session_id, role, content, None, None)
            .await
    }

    async fn add_message_with_tool(
        &self,
        session_id: i64,
        role: &str,
        content: impl Into<String>,
        tool_calls: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> Result<(), ContextError> {
        // Keep the persisted conversation OpenAI-shaped: a `tool` result must
        // reference the call it answers, and only an assistant message may
        // carry tool calls — never both on one row (PR #466 review).
        debug_assert!(match role {
            "tool" => tool_call_id.is_some() && tool_calls.is_none(),
            "assistant" => tool_call_id.is_none(),
            _ => tool_calls.is_none() && tool_call_id.is_none(),
        });
        self.ensure_session_exists(session_id).await?;

        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(session_id)
        .bind(role)
        .bind(content.into())
        .bind(tool_calls)
        .bind(tool_call_id)
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

    /// Highest message id persisted for the session (0 when empty).
    ///
    /// Used by the OpenAI surface as the rollback baseline for a request's
    /// writes, so a failed turn can remove exactly the messages it added
    /// (PR #466 review).
    pub async fn max_message_id(&self, session_id: i64) -> Result<i64, ContextError> {
        self.ensure_session_exists(session_id).await?;
        let id: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM messages WHERE session_id = ?1")
                .bind(session_id)
                .fetch_one(self.pool.as_ref())
                .await?;
        Ok(id)
    }

    /// Delete every message persisted after `after_id` for the session.
    ///
    /// Restores a session to its pre-request state when an OpenAI turn fails
    /// before completion; the caller holds the per-session permit, so no
    /// concurrent request can interleave writes for the same session
    /// (PR #466 review).
    pub async fn delete_messages_after(
        &self,
        session_id: i64,
        after_id: i64,
    ) -> Result<u64, ContextError> {
        self.ensure_session_exists(session_id).await?;
        let result = sqlx::query("DELETE FROM messages WHERE session_id = ?1 AND id > ?2")
            .bind(session_id)
            .bind(after_id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

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
                SELECT id, session_id, role, content, tool_calls, tool_call_id, created_at, token_count
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
}
