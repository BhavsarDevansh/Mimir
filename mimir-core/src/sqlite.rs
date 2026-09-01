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
