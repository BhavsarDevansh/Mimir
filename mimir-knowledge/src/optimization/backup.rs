//! Database backup creation and retention pruning for optimization runs.

use std::path::Path;

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
                let meta = match entry.metadata().await {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                // A reserved-but-never-written backup (crash between the
                // atomic filename reservation and `VACUUM INTO`) is empty;
                // never count it or let it displace a real backup from the
                // keep-newest window.
                if meta.len() == 0 {
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
        // reserved file is empty, so `prune_backups` never counts it, and the
        // completed backup is later published over it with an atomic rename.
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
        // Write the backup to a staging path that `prune_backups` does not
        // match (`knowledge-*.db`), so a concurrent pruning pass can never
        // unlink an in-progress backup; publish the completed file to the
        // reserved `.db` path only after `VACUUM INTO` succeeds.
        let staging = backup.with_extension("db.staging");
        // A staging file can only be an orphan of a crashed run whose
        // reservation was already cleaned up, so clear it before `VACUUM
        // INTO`, which refuses to overwrite a non-empty file.
        match tokio::fs::remove_file(&staging).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let escaped = staging.to_string_lossy().replace('\'', "''");
        let result = sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{}'", escaped)))
            .execute(self.kg.pool())
            .await;
        if result.is_err() {
            // Do not leave the reserved empty file or a partial staging file
            // behind: the empty reservation would burn the filename, and a
            // partial staging file would be orphaned forever.
            discard_backup_artifacts(&backup, &staging).await;
        }
        result?;
        // Publish the completed backup by renaming over the empty reservation
        // (atomic on Unix).
        if let Err(e) = tokio::fs::rename(&staging, &backup).await {
            discard_backup_artifacts(&backup, &staging).await;
            return Err(e.into());
        }
        Ok(())
    }
}

/// Remove the reserved destination and its staging file, ignoring files that
/// are already gone. The reservation is removed first so a crash mid-cleanup
/// leaves only a staging orphan, which the next run clears before `VACUUM
/// INTO`.
async fn discard_backup_artifacts(backup: &Path, staging: &Path) {
    for path in [backup, staging] {
        if let Err(e) = tokio::fs::remove_file(path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Failed to remove backup artifact {}: {}", path.display(), e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KnowledgeGraph;
    use crate::clock::MockClock;
    use crate::optimization::OptimizationConfig;
    use chrono::{DateTime, Utc};
    use std::sync::Arc;

    async fn kg_with_clock(dir: &tempfile::TempDir) -> KnowledgeGraph {
        let fixed = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        KnowledgeGraph::init_with_clock(
            &dir.path().join("source.db"),
            Arc::new(MockClock::new(fixed)),
        )
        .await
        .unwrap()
    }

    fn runner(kg: &KnowledgeGraph, backup_dir: std::path::PathBuf) -> OptimizationRunner<'_> {
        OptimizationRunner::new(kg, OptimizationConfig::for_test(backup_dir), None)
    }

    async fn backup_file_names(backup_dir: &std::path::Path) -> Vec<String> {
        let mut entries = tokio::fs::read_dir(backup_dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        names.sort();
        names
    }

    #[tokio::test]
    async fn create_backup_publishes_completed_db_without_staging_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let kg = kg_with_clock(&dir).await;
        let backup_dir = dir.path().join("backups");

        runner(&kg, backup_dir.clone())
            .create_backup()
            .await
            .unwrap();

        let names = backup_file_names(&backup_dir).await;
        assert_eq!(names, vec!["knowledge-2024-03-15.db"]);

        // The published file must be a complete, queryable database, not a
        // partially written file exposed under the final `.db` name.
        let pool = sqlx::SqlitePool::connect(&format!(
            "sqlite://{}",
            backup_dir.join("knowledge-2024-03-15.db").display()
        ))
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn create_backup_clears_orphaned_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let kg = kg_with_clock(&dir).await;
        let backup_dir = dir.path().join("backups");
        tokio::fs::create_dir_all(&backup_dir).await.unwrap();
        // A crashed run whose reservation was already cleaned up leaves only
        // its staging file behind; the next run must clear it before
        // `VACUUM INTO`, which refuses to overwrite a non-empty file.
        tokio::fs::write(
            backup_dir.join("knowledge-2024-03-15.db.staging"),
            b"partial",
        )
        .await
        .unwrap();

        runner(&kg, backup_dir.clone())
            .create_backup()
            .await
            .unwrap();

        let names = backup_file_names(&backup_dir).await;
        assert_eq!(names, vec!["knowledge-2024-03-15.db"]);
    }

    #[tokio::test]
    async fn prune_backups_never_removes_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        let kg = kg_with_clock(&dir).await;
        let backup_dir = dir.path().join("backups");
        tokio::fs::create_dir_all(&backup_dir).await.unwrap();
        // Seven completed backups fill the keep-newest window; an in-progress
        // backup lives in a staging file that pruning must never match.
        for i in 0..7 {
            tokio::fs::write(
                backup_dir.join(format!("knowledge-2024-03-1{i}.db")),
                b"sqlite",
            )
            .await
            .unwrap();
        }
        let staging = backup_dir.join("knowledge-2024-03-15.db.staging");
        tokio::fs::write(&staging, b"partial").await.unwrap();

        runner(&kg, backup_dir.clone())
            .prune_backups()
            .await
            .unwrap();

        assert!(
            staging.exists(),
            "prune must never remove in-progress staging files"
        );
        let names = backup_file_names(&backup_dir).await;
        assert_eq!(
            names.len(),
            8,
            "all completed backups and the staging file must survive"
        );
    }
}
