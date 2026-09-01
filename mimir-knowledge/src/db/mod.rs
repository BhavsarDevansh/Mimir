//! Database connection helpers and pragma configuration.

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::path::Path;
use std::str::FromStr;

/// Create a connection pool for the given SQLite database path.
pub async fn create_pool(db_path: &Path) -> sqlx::Result<SqlitePool> {
    let path_str = db_path.to_string_lossy();
    let opts = SqliteConnectOptions::from_str(&path_str)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .pragma("cache_size", "10000")
        .foreign_keys(true)
        .optimize_on_close(true, 400)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    Ok(pool)
}
