//! Shared SQLite connection settings for locally-owned Mimir databases.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use std::path::Path;

const PAGE_CACHE_SIZE: i64 = 10_000;

/// Build the shared connection settings for a locally-owned Mimir database.
pub(crate) fn connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .pragma("cache_size", PAGE_CACHE_SIZE.to_string())
}

#[cfg(test)]
/// Assert that a pooled connection exposes the shared SQLite pragmas.
pub(crate) async fn assert_connection_pragmas(pool: &sqlx::SqlitePool) {
    let (journal_mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(journal_mode.to_lowercase(), "wal");

    let (synchronous,): (i64,) = sqlx::query_as("PRAGMA synchronous")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(synchronous, 1, "WAL databases must use synchronous=NORMAL");

    let (cache_size,): (i64,) = sqlx::query_as("PRAGMA cache_size")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        cache_size, PAGE_CACHE_SIZE,
        "SQLite page cache must be preconfigured"
    );
}
