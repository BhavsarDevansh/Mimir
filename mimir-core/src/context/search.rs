//! Full-text search over conversation messages.

use crate::context::ContextManager;
use crate::context::{ContextError, MessageSearchResult};
use crate::fts5::escape_fts5_tokens;
use sqlx::Row;

/// Tokens of context to show on each side of a match in a search snippet.
const SNIPPET_SIDE_TOKENS: usize = 30;

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

        // `snippet()` centres the fragment on the first match, so fetching
        // 1000 tokens guarantees at least `SNIPPET_SIDE_TOKENS` of context on
        // each side after `trim_snippet_window` cuts the window down.
        let rows = if let Some(sid) = session_id {
            sqlx::query(
                r#"
                SELECT m.session_id, m.role, m.created_at,
                       snippet(messages_fts, -1, '<<<', '>>>', '...', 1000) as snippet
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
                       snippet(messages_fts, -1, '<<<', '>>>', '...', 1000) as snippet
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
            let raw_snippet: String = row.try_get("snippet")?;
            results.push(MessageSearchResult {
                session_id: row.try_get("session_id")?,
                role: row.try_get("role")?,
                created_at: row.try_get("created_at")?,
                snippet: trim_snippet_window(&raw_snippet, SNIPPET_SIDE_TOKENS),
            });
        }
        Ok(results)
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------
}

/// Trim a SQLite `snippet()` fragment to at most `side_tokens` tokens on each
/// side of the first match, preserving the `<<<`/`>>>` markers and adding `...`
/// at any new cut boundary. Fragments without markers are returned unchanged.
fn trim_snippet_window(fragment: &str, side_tokens: usize) -> String {
    let tokens: Vec<&str> = fragment.split_whitespace().collect();
    let start = tokens.iter().position(|t| t.contains("<<<"));
    let end = start.and_then(|s| {
        tokens[s..]
            .iter()
            .position(|t| t.contains(">>>"))
            .map(|i| s + i)
    });
    let (Some(start), Some(end)) = (start, end) else {
        return fragment.to_string();
    };
    let left = start.saturating_sub(side_tokens);
    let right = (end + 1 + side_tokens).min(tokens.len());
    let mut out = String::new();
    if left > 0 {
        out.push_str("... ");
    }
    out.push_str(&tokens[left..right].join(" "));
    if right < tokens.len() {
        out.push_str(" ...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::trim_snippet_window;

    #[test]
    fn trim_snippet_window_keeps_side_tokens_and_adds_ellipses() {
        let fragment = "word01 word02 word03 <<<needle>>> post01 post02 post03";
        assert_eq!(
            trim_snippet_window(fragment, 1),
            "... word03 <<<needle>>> post01 ..."
        );
    }

    #[test]
    fn trim_snippet_window_keeps_whole_short_fragment() {
        let fragment = "hello <<<needle>>> world";
        assert_eq!(trim_snippet_window(fragment, 30), fragment);
    }

    #[test]
    fn trim_snippet_window_handles_phrase_markers() {
        let fragment = "pre01 pre02 <<<check in>>> post01 post02";
        assert_eq!(
            trim_snippet_window(fragment, 1),
            "... pre02 <<<check in>>> post01 ..."
        );
    }

    #[test]
    fn trim_snippet_window_returns_fragment_without_markers_unchanged() {
        let fragment = "no markers here";
        assert_eq!(trim_snippet_window(fragment, 30), fragment);
    }
}
