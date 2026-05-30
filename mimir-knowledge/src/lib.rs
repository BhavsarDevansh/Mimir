//! `mimir-knowledge` — SQLite-based knowledge graph for Mimir.
//!
//! Provides entity and fact storage, temporal queries, provenance tracking,
//! and full-text search via SQLite FTS5.

pub mod clock;
pub mod db;
pub mod extract;
pub mod inference;
pub mod models;
pub mod optimization;
pub mod queries;

use clock::{Clock, RealClock};
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::Arc;

/// Errors that can occur during knowledge graph initialization or operation.
#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("Database pool creation failed: {0}")]
    Pool(#[from] sqlx::Error),

    #[error("I/O error preparing database path: {0}")]
    Io(#[from] std::io::Error),

    #[error("Migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}

/// The public API for the knowledge graph.
///
/// Holds a SQLite connection pool and a clock for deterministic timestamps
/// in tests.
pub struct KnowledgeGraph {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
}

impl KnowledgeGraph {
    /// Initialise the knowledge graph: ensure parent directories exist, open
    /// the SQLite pool (WAL + foreign keys), and run pending migrations.
    pub async fn init(db_path: &Path) -> Result<Self, KnowledgeError> {
        Self::init_with_clock(db_path, Arc::new(RealClock)).await
    }

    /// Initialise with a custom clock (used in tests for determinism).
    pub async fn init_with_clock(
        db_path: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, KnowledgeError> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let pool = db::create_pool(db_path).await?;
        sqlx::migrate!("src/db/migrations").run(&pool).await?;

        Ok(Self { pool, clock })
    }

    /// Access the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Current timestamp according to the configured clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }
}
