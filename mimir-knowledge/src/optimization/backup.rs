//! Database backup creation and retention pruning for optimization runs.

use super::OptimizationRunner;

impl<'a> OptimizationRunner<'a> {
    pub(super) async fn prune_backups(&self) -> Result<(), crate::KnowledgeError> {
        let mut entries = tokio::fs::read_dir(&self.config.backup_dir).await?;
        let mut backups = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("knowledge-") && name.ends_with(".db") {
                // A concurrent run may prune the same file between
                // `next_entry` and `metadata`; skip entries that vanish
                // mid-scan instead of failing the whole pass (issue #241).
                let Ok(meta) = entry.metadata().await else {
                    continue;
                };
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
        // Reserve the filename atomically (O_EXCL) so concurrent runs sharing
        // a backup directory can never pick the same file (issue #241). The
        // reserved file is empty, which `VACUUM INTO` is allowed to overwrite.
        let mut backup = self
            .config
            .backup_dir
            .join(format!("knowledge-{}.db", date));
        let mut counter = 1u32;
        loop {
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&backup)
                .await
            {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    backup = self
                        .config
                        .backup_dir
                        .join(format!("knowledge-{}-{}.db", date, counter));
                    counter += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
        let escaped = backup.to_string_lossy().replace('\'', "''");
        let result = sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{}'", escaped)))
            .execute(self.kg.pool())
            .await;
        if result.is_err() {
            // Do not leave the reserved empty file behind to be counted by
            // `prune_backups` (it would displace a real backup from the
            // keep-newest window).
            let _ = tokio::fs::remove_file(&backup).await;
        }
        result?;
        Ok(())
    }
}
