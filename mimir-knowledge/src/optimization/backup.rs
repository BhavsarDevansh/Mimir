//! Database backup creation and retention pruning for optimization runs.

use super::OptimizationRunner;

impl<'a> OptimizationRunner<'a> {
    pub(super) async fn prune_backups(&self) -> Result<(), crate::KnowledgeError> {
        let mut entries = tokio::fs::read_dir(&self.config.backup_dir).await?;
        let mut backups = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("knowledge-") && name.ends_with(".db") {
                let meta = entry.metadata().await?;
                if let Ok(modified) = meta.modified() {
                    backups.push((name, modified));
                }
            }
        }
        backups.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (name, _) in backups.into_iter().skip(7) {
            let path = self.config.backup_dir.join(&name);
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!("Failed to remove old backup {}: {}", path.display(), e);
            }
        }
        Ok(())
    }

    pub(super) async fn create_backup(&self) -> Result<(), crate::KnowledgeError> {
        tokio::fs::create_dir_all(&self.config.backup_dir).await?;
        let date = self.kg.now().date_naive();
        let mut backup = self
            .config
            .backup_dir
            .join(format!("knowledge-{}.db", date));
        let mut counter = 1u32;
        while tokio::fs::try_exists(&backup).await.unwrap_or(false) {
            backup = self
                .config
                .backup_dir
                .join(format!("knowledge-{}-{}.db", date, counter));
            counter += 1;
        }
        let escaped = backup.to_string_lossy().replace('\'', "''");
        sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{}'", escaped)))
            .execute(self.kg.pool())
            .await?;
        Ok(())
    }
}
