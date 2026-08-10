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
