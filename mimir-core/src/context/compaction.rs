//! Compaction primitives on persisted sessions (issue #279).
//!
//! `compaction_candidates` snapshots the oldest complete turns beyond the
//! compaction window; `apply_compaction` writes the summary, advances
//! `compacted_at`, and deletes the summarised messages. The LLM
//! summarisation lives in [`compactor`] (the [`SessionCompactor`]
//! orchestrator); this module stays deterministic and DB-only so the read
//! path can be unit-tested without an LLM.

use crate::context::trim::{TurnRow, delete_message_ids, split_complete_turns};
use crate::context::{ContextError, ContextManager, ContextMessage};
use chrono::{DateTime, Utc};
use tracing::info;

/// Snapshot of the turns selected for one compaction run.
#[derive(Debug, Clone)]
pub struct CompactionCandidates {
    /// Messages of the oldest complete turns beyond the window, in
    /// chronological order. The system prompt is never included.
    pub turn_messages: Vec<ContextMessage>,
    /// Number of complete turns in `turn_messages`.
    pub compacted_turns: usize,
    /// Message ids that `apply_compaction` deletes with this batch.
    pub delete_ids: Vec<i64>,
    /// Summary already stored on the session, folded into the next one.
    pub existing_summary: Option<String>,
}

impl ContextManager {
    /// Select the oldest complete turns beyond the compaction window.
    ///
    /// A turn spans from a user message up to (excluding) the next user
    /// message, so assistant tool-call messages and tool results travel with
    /// their turn (issue #388). The in-flight final turn (no plain assistant
    /// reply yet) is never selected, and the system prompt is never
    /// included. Returns `None` when the session has nothing to compact.
    pub async fn compaction_candidates(
        &self,
        session_id: i64,
        max_turns: u16,
    ) -> Result<Option<CompactionCandidates>, ContextError> {
        self.ensure_session_exists(session_id).await?;

        let rows: Vec<TurnRow> = sqlx::query_as(
            r#"
            SELECT id, role, content, tool_calls, tool_call_id, created_at, token_count
            FROM messages
            WHERE session_id = ?1 AND role != 'system'
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let turns = split_complete_turns(&rows);
        if turns.len() <= max_turns as usize {
            return Ok(None);
        }
        let excess = turns.len() - max_turns as usize;
        let selected: Vec<&TurnRow> = turns.iter().take(excess).flatten().copied().collect();
        let compacted_turns = selected.iter().filter(|row| row.role == "user").count();

        let existing_summary: Option<String> =
            sqlx::query_scalar("SELECT summary FROM sessions WHERE id = ?1")
                .bind(session_id)
                .fetch_one(self.pool.as_ref())
                .await?;

        let turn_messages = selected
            .iter()
            .map(|row| ContextMessage {
                id: row.id,
                session_id,
                role: row.role.clone(),
                content: row.content.clone(),
                tool_calls: row.tool_calls.clone(),
                tool_call_id: row.tool_call_id.clone(),
                created_at: row.created_at,
                token_count: row.token_count.map(|t| t as u32),
            })
            .collect();

        Ok(Some(CompactionCandidates {
            turn_messages,
            compacted_turns,
            delete_ids: selected.iter().map(|row| row.id).collect(),
            existing_summary,
        }))
    }

    /// Commit a compaction: store the summary, advance `compacted_at`, and
    /// delete the summarised messages.
    ///
    /// `compacted_at` is the timestamp of the last message in the batch, so
    /// [`get_messages_after_compaction`](Self::get_messages_after_compaction)
    /// keeps exactly the retained window. Re-applying an already-deleted
    /// batch (e.g. a concurrent trim removed the rows while the LLM ran) is
    /// a no-op for the deletes and safe for the summary write. Deletes are
    /// scoped to `session_id`, so a stale batch can never touch another
    /// session's rows.
    pub async fn apply_compaction(
        &self,
        session_id: i64,
        summary: &str,
        compacted_at: DateTime<Utc>,
        delete_ids: &[i64],
    ) -> Result<(), ContextError> {
        self.ensure_session_exists(session_id).await?;

        // The summary write and the message deletes commit atomically: a
        // failure part-way can never leave a new `summary`/`compacted_at`
        // alongside some summarised messages that still exist (PR #505
        // review).
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            UPDATE sessions
            SET summary = ?1, compacted_at = ?2, updated_at = ?3
            WHERE id = ?4
            "#,
        )
        .bind(summary)
        .bind(compacted_at)
        .bind(Utc::now())
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        delete_message_ids(&mut *tx, session_id, delete_ids).await?;
        tx.commit().await?;

        info!(
            session_id,
            deleted_messages = delete_ids.len(),
            "session compaction applied"
        );
        Ok(())
    }
}
