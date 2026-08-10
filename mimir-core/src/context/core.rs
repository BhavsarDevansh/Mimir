//! Context manager core: construction and shutdown.

use crate::context::ContextError;
use crate::context::ContextManager;
use crate::context::path::expand_tilde;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

impl ContextManager {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, ContextError> {
        let path = expand_tilde(db_path.as_ref());

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;

        sqlx::query("PRAGMA journal_mode = WAL;")
            .execute(&pool)
            .await?;

        Self::init_schema(&pool).await?;

        #[cfg(unix)]
        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = path.parent() {
                let perms = Permissions::from_mode(0o700);
                tokio::fs::set_permissions(parent, perms).await?;
            }
        }

        info!(db_path = %path.display(), "ContextManager initialised");
        Ok(Self {
            pool: Arc::new(pool),
            sessions: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}
