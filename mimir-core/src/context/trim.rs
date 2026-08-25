//! Context-budget trimming: turn/token caps with system-prompt preservation.

use crate::context::ContextError;
use crate::context::ContextManager;
use chrono::{DateTime, Utc};
use tracing::{debug, warn};

/// A non-system conversation message row used by the turn-boundary helpers.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct TurnRow {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<String>,
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub token_count: Option<i64>,
}

/// Split non-system rows into complete turns.
///
/// A turn spans from a user message up to (but excluding) the next user
/// message, so assistant tool-call messages and tool results belong to their
/// turn (issue #388). Rows before the first user message cannot be
/// attributed and are skipped. The final turn is excluded as in-flight when
/// the last row is not a plain assistant message (no reply yet, or an
/// assistant tool-call message still awaiting the client's tool results) —
/// it must never be trimmed or compacted before the LLM call (issue #388,
/// PR #466 review). The system prompt is filtered out by callers.
pub(super) fn split_complete_turns(rows: &[TurnRow]) -> Vec<Vec<&TurnRow>> {
    let mut turns: Vec<Vec<&TurnRow>> = Vec::new();
    let mut current: Vec<&TurnRow> = Vec::new();
    for row in rows {
        if row.role == "user" {
            if !current.is_empty() {
                turns.push(std::mem::take(&mut current));
            }
            current.push(row);
        } else if !current.is_empty() {
            current.push(row);
        }
    }
    let final_turn_complete = rows
        .last()
        .is_some_and(|row| row.role == "assistant" && row.tool_calls.is_none());
    if final_turn_complete && !current.is_empty() {
        turns.push(current);
    }
    turns
}

impl ContextManager {
    pub async fn trim_to_budget(
        &self,
        session_id: i64,
        max_tokens: Option<u32>,
        max_turns: u16,
    ) -> Result<(), ContextError> {
        self.ensure_session_exists(session_id).await?;

        // ---- Hard turn cap ----
        // A turn is delimited by user messages, so the user-message count is
        // the turn count even when tool messages are interleaved (issue #388).
        let turn_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND role = 'user'",
        )
        .bind(session_id)
        .fetch_one(self.pool.as_ref())
        .await?;

        if turn_count > (max_turns as i64) {
            let excess = turn_count - (max_turns as i64);
            warn!(
                session_id = %session_id,
                excess,
                "trimming oldest turns (turn cap)"
            );
            self.delete_oldest_turns(session_id, excess).await?;
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
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND role NOT IN ('system', 'tool') AND token_count IS NULL"
            )
            .bind(session_id)
            .fetch_one(self.pool.as_ref())
            .await?;

            if unknown_count > 0 {
                // `tool`-role messages never carry token counts (usage is
                // attributed only to user/assistant messages), so they are
                // excluded from the probe; otherwise a single tool round-trip
                // would force the conservative fallback forever (PR #466 review).
                // Some or all messages lack token counts: conservative fallback.
                let target_turns = (max_turns as i64) / 2;
                let current_turns: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND role = 'user'",
                )
                .bind(session_id)
                .fetch_one(self.pool.as_ref())
                .await?;
                if current_turns > target_turns {
                    self.delete_oldest_turns(session_id, current_turns - target_turns)
                        .await?;
                }
            } else {
                // All messages have known token counts: trim by token sum.
                let to_remove = self
                    .count_turns_to_remove_by_tokens(session_id, budget)
                    .await?;
                if to_remove > 0 {
                    self.delete_oldest_turns(session_id, to_remove).await?;
                }
            }
        }

        Ok(())
    }

    /// Count how many oldest turns must be removed to fit `max_tokens`.
    ///
    /// Turn boundaries and the in-flight final-turn rule come from
    /// [`split_complete_turns`]; only the per-turn token sums are computed
    /// here (issue #388).
    async fn count_turns_to_remove_by_tokens(
        &self,
        session_id: i64,
        max_tokens: u32,
    ) -> Result<i64, ContextError> {
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

        let total: i64 = rows.iter().filter_map(|row| row.token_count).sum();
        if total <= (max_tokens as i64) {
            return Ok(0);
        }

        let turn_tokens: Vec<i64> = split_complete_turns(&rows)
            .iter()
            .map(|turn| turn.iter().filter_map(|row| row.token_count).sum())
            .collect();

        let mut running = 0i64;
        for (index, tokens) in turn_tokens.iter().enumerate() {
            running += tokens;
            if total - running <= (max_tokens as i64) {
                return Ok((index + 1) as i64);
            }
        }

        Ok(turn_tokens.len() as i64)
    }

    /// Delete the oldest `n` complete turns.
    ///
    /// A turn spans from a user message up to (but excluding) the next user
    /// message, so assistant tool-call messages and tool results are removed
    /// with their turn instead of being orphaned (issue #388).
    async fn delete_oldest_turns(&self, session_id: i64, n: i64) -> Result<(), ContextError> {
        if n <= 0 {
            return Ok(());
        }

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
        let to_delete: Vec<&TurnRow> = turns.iter().take(n as usize).flatten().copied().collect();
        let turns_removed = to_delete.iter().filter(|row| row.role == "user").count();

        for row in &to_delete {
            sqlx::query("DELETE FROM messages WHERE id = ?1")
                .bind(row.id)
                .execute(self.pool.as_ref())
                .await?;
        }

        debug!(session_id = %session_id, turns_removed, "deleted oldest turns");
        Ok(())
    }
}
