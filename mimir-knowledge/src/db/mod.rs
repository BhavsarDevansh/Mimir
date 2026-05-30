//! Database connection helpers and pragma configuration.

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;

/// Create a connection pool for the given SQLite database path.
pub async fn create_pool(db_path: &Path) -> sqlx::Result<SqlitePool> {
    let path_str = db_path.to_string_lossy();
    let opts = SqliteConnectOptions::from_str(&path_str)?
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    // Double-check pragmas after connect (WAL is set per-connection).
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;

    Ok(pool)
}
