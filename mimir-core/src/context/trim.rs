//! Context-budget trimming: turn/token caps with system-prompt preservation.

use crate::context::ContextError;
use crate::context::ContextManager;
use tracing::{debug, warn};

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
    /// A turn is every message from a user message up to (but excluding) the
    /// next user message, so assistant tool-call messages and tool results
    /// are attributed to their turn (issue #388).
    async fn count_turns_to_remove_by_tokens(
        &self,
        session_id: i64,
        max_tokens: u32,
    ) -> Result<i64, ContextError> {
        let rows: Vec<(i64, String, Option<String>, Option<i64>)> = sqlx::query_as(
            r#"
            SELECT id, role, tool_calls, token_count FROM messages
            WHERE session_id = ?1 AND role != 'system'
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let total: i64 = rows.iter().filter_map(|(_, _, _, t)| *t).sum();
        if total <= (max_tokens as i64) {
            return Ok(0);
        }

        let mut turn_tokens: Vec<i64> = Vec::new();
        let mut current = 0i64;
        let mut in_turn = false;
        for (_id, role, _tool_calls, tokens) in &rows {
            if role == "user" {
                if in_turn {
                    turn_tokens.push(current);
                }
                current = 0;
                in_turn = true;
            }
            if in_turn {
                current += tokens.unwrap_or(0);
            }
        }
        if in_turn {
            turn_tokens.push(current);
        }

        // The final turn is the one being answered right now: its user
        // message (or trailing tool results) was just persisted and must
        // never be trimmed away before the LLM call. A turn is complete only
        // when the final row is a plain assistant message — an assistant
        // tool-call message still awaits the client's tool results, so it is
        // in-flight too (issue #388, PR #466 review).
        let final_turn_complete = rows
            .last()
            .is_some_and(|(_, role, tool_calls, _)| role == "assistant" && tool_calls.is_none());
        if !final_turn_complete {
            turn_tokens.pop();
        }

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

        let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT id, role, tool_calls FROM messages
            WHERE session_id = ?1 AND role != 'system'
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut ids_to_delete = Vec::new();
        let mut turns_found = 0i64;
        let mut turns_removed = 0i64;
        let mut in_turn = false;
        // Index of the final turn's user message in `rows`; the final turn
        // is in-flight when the session's last message is not a plain
        // assistant reply (no reply yet, or an assistant tool-call message
        // still awaiting the client's tool results).
        let final_turn_start = rows
            .iter()
            .rposition(|(_, role, _)| role == "user")
            .unwrap_or(usize::MAX);
        let protect_final_turn = rows
            .last()
            .is_some_and(|(_, role, tool_calls)| role != "assistant" || tool_calls.is_some());

        for (index, (id, role, _)) in rows.iter().enumerate() {
            if role == "user" {
                if turns_found >= n {
                    break;
                }
                turns_found += 1;
                in_turn = true;
            }
            if in_turn {
                // The final turn is the in-flight turn being answered right
                // now (its user message or tool results were just persisted);
                // it has no assistant reply yet, so it must never be deleted
                // mid-request (issue #388).
                if protect_final_turn && index >= final_turn_start {
                    break;
                }
                ids_to_delete.push(*id);
                if role == "user" {
                    turns_removed += 1;
                }
            }
        }

        for id in ids_to_delete {
            sqlx::query("DELETE FROM messages WHERE id = ?1")
                .bind(id)
                .execute(self.pool.as_ref())
                .await?;
        }

        debug!(session_id = %session_id, turns_removed, "deleted oldest turns");
        Ok(())
    }
}
