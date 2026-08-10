//! Database schema initialisation and migrations.

use crate::context::{ContextError, ContextManager};
use chrono::{DateTime, Utc};
use sqlx::{Acquire, SqlitePool};
use tracing::info;

impl ContextManager {
    pub(super) async fn init_schema(pool: &SqlitePool) -> Result<(), ContextError> {
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
        let fts_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages_fts'",
        )
        .fetch_one(pool)
        .await?;

        if fts_exists == 0 {
            sqlx::query(
                r#"
                CREATE VIRTUAL TABLE messages_fts USING fts5(
                    role,
                    content,
                    content='messages',
                    content_rowid='id'
                )
                "#,
            )
            .execute(pool)
            .await?;

            // Backfill index for existing messages (only needed on first creation).
            sqlx::query("INSERT INTO messages_fts(messages_fts) VALUES('rebuild');")
                .execute(pool)
                .await?;
        }

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
