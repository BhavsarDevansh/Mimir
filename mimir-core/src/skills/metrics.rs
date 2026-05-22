use super::SkillError;
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// Tracks invocation metrics for skills in a dedicated SQLite database.
pub struct SkillMetricsDb {
    pool: Arc<SqlitePool>,
}

impl SkillMetricsDb {
    /// Open (or create) the metrics database at the given path.
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, SkillError> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                SkillError::metrics_error("init", format!("failed to create parent dirs: {e}"))
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await.map_err(|e| {
            SkillError::metrics_error("init", format!("sqlite connect failed: {e}"))
        })?;

        Self::init_schema(&pool).await?;
        info!(db_path = %path.display(), "SkillMetricsDb initialised");
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    async fn init_schema(pool: &SqlitePool) -> Result<(), SkillError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS skill_metrics (
                skill_name TEXT PRIMARY KEY,
                invocation_count INTEGER DEFAULT 0 NOT NULL,
                success_count INTEGER DEFAULT 0 NOT NULL,
                failure_count INTEGER DEFAULT 0 NOT NULL,
                avg_latency_ms INTEGER DEFAULT 0 NOT NULL,
                last_invoked_at DATETIME,
                user_correction_count INTEGER DEFAULT 0 NOT NULL,
                avg_token_cost INTEGER DEFAULT 0 NOT NULL,
                priced_count INTEGER DEFAULT 0 NOT NULL,
                utility_score REAL
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| SkillError::metrics_error("init_schema", e.to_string()))?;

        // Migration: add priced_count if it does not exist (no-op on new DBs).
        let _ = sqlx::query(
            "ALTER TABLE skill_metrics ADD COLUMN IF NOT EXISTS priced_count INTEGER DEFAULT 0 NOT NULL"
        )
        .execute(pool)
        .await;

        Ok(())
    }

    /// Record an invocation for a skill.
    pub async fn record_invocation(
        &self,
        skill_name: &str,
        success: bool,
        latency_ms: u64,
        token_cost: Option<u32>,
    ) -> Result<(), SkillError> {
        let now = chrono::Utc::now();

        let success_delta: i64 = if success { 1 } else { 0 };
        let failure_delta: i64 = if success { 0 } else { 1 };
        let latency_i64 = latency_ms as i64;
        let token_i64 = token_cost.map(|v| v as i64).unwrap_or(0);
        let priced_delta: i64 = if token_cost.is_some() { 1 } else { 0 };

        sqlx::query(
            r#"
            INSERT INTO skill_metrics (
                skill_name, invocation_count, success_count, failure_count,
                avg_latency_ms, last_invoked_at, avg_token_cost, priced_count
            ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(skill_name) DO UPDATE SET
                invocation_count = skill_metrics.invocation_count + 1,
                success_count = skill_metrics.success_count + excluded.success_count,
                failure_count = skill_metrics.failure_count + excluded.failure_count,
                avg_latency_ms = (
                    (skill_metrics.avg_latency_ms * skill_metrics.invocation_count)
                    + excluded.avg_latency_ms
                ) / (skill_metrics.invocation_count + 1),
                last_invoked_at = excluded.last_invoked_at,
                avg_token_cost = CASE
                    WHEN excluded.priced_count > 0 THEN
                        (
                            (skill_metrics.avg_token_cost * skill_metrics.priced_count)
                            + excluded.avg_token_cost
                        ) / (skill_metrics.priced_count + 1)
                    ELSE skill_metrics.avg_token_cost
                END,
                priced_count = skill_metrics.priced_count + excluded.priced_count
            "#,
        )
        .bind(skill_name)
        .bind(success_delta)
        .bind(failure_delta)
        .bind(latency_i64)
        .bind(now)
        .bind(token_i64)
        .bind(priced_delta)
        .execute(self.pool.as_ref())
        .await
        .map_err(|e| SkillError::metrics_error(skill_name, e.to_string()))?;

        Ok(())
    }

    /// Get metrics for a single skill.
    pub async fn get_metrics(&self, skill_name: &str) -> Result<Option<SkillMetrics>, SkillError> {
        let row = sqlx::query("SELECT * FROM skill_metrics WHERE skill_name = ?1")
            .bind(skill_name)
            .fetch_optional(self.pool.as_ref())
            .await
            .map_err(|e| SkillError::metrics_error(skill_name, e.to_string()))?;

        match row {
            Some(row) => Ok(Some(SkillMetrics {
                skill_name: row.try_get("skill_name").unwrap_or_default(),
                invocation_count: row.try_get::<i64, _>("invocation_count").unwrap_or(0) as u64,
                success_count: row.try_get::<i64, _>("success_count").unwrap_or(0) as u64,
                failure_count: row.try_get::<i64, _>("failure_count").unwrap_or(0) as u64,
                avg_latency_ms: row.try_get::<i64, _>("avg_latency_ms").unwrap_or(0) as u64,
                last_invoked_at: row.try_get("last_invoked_at").ok(),
                user_correction_count: row.try_get::<i64, _>("user_correction_count").unwrap_or(0)
                    as u64,
                avg_token_cost: row.try_get::<i64, _>("avg_token_cost").unwrap_or(0) as u64,
                utility_score: row.try_get::<f64, _>("utility_score").ok(),
            })),
            None => Ok(None),
        }
    }

    /// List metrics for all skills.
    pub async fn list_metrics(&self) -> Result<Vec<SkillMetrics>, SkillError> {
        let rows = sqlx::query("SELECT * FROM skill_metrics")
            .fetch_all(self.pool.as_ref())
            .await
            .map_err(|e| SkillError::metrics_error("list", e.to_string()))?;

        let mut metrics = Vec::with_capacity(rows.len());
        for row in rows {
            metrics.push(SkillMetrics {
                skill_name: row.try_get("skill_name").unwrap_or_default(),
                invocation_count: row.try_get::<i64, _>("invocation_count").unwrap_or(0) as u64,
                success_count: row.try_get::<i64, _>("success_count").unwrap_or(0) as u64,
                failure_count: row.try_get::<i64, _>("failure_count").unwrap_or(0) as u64,
                avg_latency_ms: row.try_get::<i64, _>("avg_latency_ms").unwrap_or(0) as u64,
                last_invoked_at: row.try_get("last_invoked_at").ok(),
                user_correction_count: row.try_get::<i64, _>("user_correction_count").unwrap_or(0)
                    as u64,
                avg_token_cost: row.try_get::<i64, _>("avg_token_cost").unwrap_or(0) as u64,
                utility_score: row.try_get::<f64, _>("utility_score").ok(),
            });
        }
        Ok(metrics)
    }

    /// Reset metrics for a skill.
    pub async fn reset_metrics(&self, skill_name: &str) -> Result<(), SkillError> {
        sqlx::query("DELETE FROM skill_metrics WHERE skill_name = ?1")
            .bind(skill_name)
            .execute(self.pool.as_ref())
            .await
            .map_err(|e| SkillError::metrics_error(skill_name, e.to_string()))?;
        Ok(())
    }
}

/// A snapshot of metrics for a single skill.
#[derive(Debug, Clone)]
pub struct SkillMetrics {
    pub skill_name: String,
    pub invocation_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub avg_latency_ms: u64,
    pub last_invoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub user_correction_count: u64,
    pub avg_token_cost: u64,
    pub utility_score: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> (SkillMetricsDb, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("skills.db");
        let metrics = SkillMetricsDb::new(&db).await.unwrap();
        (metrics, dir)
    }

    #[tokio::test]
    async fn record_invocation_creates_row() {
        let (db, _dir) = setup_db().await;
        db.record_invocation("test_skill", true, 100, Some(50))
            .await
            .unwrap();

        let m = db.get_metrics("test_skill").await.unwrap().unwrap();
        assert_eq!(m.skill_name, "test_skill");
        assert_eq!(m.invocation_count, 1);
        assert_eq!(m.success_count, 1);
        assert_eq!(m.failure_count, 0);
        assert_eq!(m.avg_latency_ms, 100);
        assert_eq!(m.avg_token_cost, 50);
        assert!(m.last_invoked_at.is_some());
    }

    #[tokio::test]
    async fn record_multiple_invocations_updates_averages() {
        let (db, _dir) = setup_db().await;
        db.record_invocation("test_skill", true, 100, Some(50))
            .await
            .unwrap();
        db.record_invocation("test_skill", false, 200, Some(100))
            .await
            .unwrap();

        let m = db.get_metrics("test_skill").await.unwrap().unwrap();
        assert_eq!(m.invocation_count, 2);
        assert_eq!(m.success_count, 1);
        assert_eq!(m.failure_count, 1);
        assert_eq!(m.avg_latency_ms, 150);
        assert_eq!(m.avg_token_cost, 75);
    }

    #[tokio::test]
    async fn record_unpriced_does_not_affect_token_cost_average() {
        let (db, _dir) = setup_db().await;
        db.record_invocation("test_skill", true, 100, Some(50))
            .await
            .unwrap();
        db.record_invocation("test_skill", true, 200, None)
            .await
            .unwrap();

        let m = db.get_metrics("test_skill").await.unwrap().unwrap();
        assert_eq!(m.invocation_count, 2);
        // avg_token_cost should remain 50 (only the priced invocation counts).
        assert_eq!(m.avg_token_cost, 50);
    }

    #[tokio::test]
    async fn list_metrics_returns_all() {
        let (db, _dir) = setup_db().await;
        db.record_invocation("skill_a", true, 10, None)
            .await
            .unwrap();
        db.record_invocation("skill_b", false, 20, None)
            .await
            .unwrap();

        let all = db.list_metrics().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn reset_metrics_removes_row() {
        let (db, _dir) = setup_db().await;
        db.record_invocation("test_skill", true, 100, None)
            .await
            .unwrap();
        db.reset_metrics("test_skill").await.unwrap();

        let m = db.get_metrics("test_skill").await.unwrap();
        assert!(m.is_none());
    }
}
