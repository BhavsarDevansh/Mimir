//! Full-text search over conversation messages.

use crate::context::ContextManager;
use crate::context::{ContextError, MessageSearchResult};
use crate::fts5::escape_fts5_tokens;
use sqlx::Row;

impl ContextManager {
    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<i64>,
    ) -> Result<Vec<MessageSearchResult>, ContextError> {
        let safe_query = escape_fts5_tokens(query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.min(100) as i64;

        let rows = if let Some(sid) = session_id {
            sqlx::query(
                r#"
                SELECT m.session_id, m.role, m.created_at,
                       snippet(messages_fts, -1, '<<<', '>>>', '...', 30) as snippet
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
                       snippet(messages_fts, -1, '<<<', '>>>', '...', 30) as snippet
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
}
