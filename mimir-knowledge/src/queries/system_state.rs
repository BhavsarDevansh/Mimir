//! Simple key/value read/write for the system_state table.

use sqlx::SqlitePool;

use crate::KnowledgeError;

/// Read a value from system_state by key.
pub async fn get_system_state(pool: &SqlitePool, key: &str) -> Result<Option<String>, KnowledgeError> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM system_state WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(value)
}

/// Write or update a value in system_state.
pub async fn set_system_state(
    pool: &SqlitePool,
    key: &str,
    value: &str,
) -> Result<(), KnowledgeError> {
    sqlx::query(
        "INSERT INTO system_state (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a system_state entry.
pub async fn delete_system_state(pool: &SqlitePool, key: &str) -> Result<(), KnowledgeError> {
    sqlx::query("DELETE FROM system_state WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}
