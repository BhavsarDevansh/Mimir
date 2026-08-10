//! SQLite-backed job queue: registration, scheduling, and execution.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use tokio::sync::RwLock;

use crate::job_queue::{
    DailySchedule, Job, JobContext, JobError, JobHandler, JobPriority, JobRunStatus, JobRunSummary,
    JobStatus,
};

#[derive(Clone)]
pub struct JobQueue {
    pool: SqlitePool,
    handlers: Arc<RwLock<HashMap<String, Arc<JobHandler>>>>,
    timeout: Arc<RwLock<Duration>>,
}

impl std::fmt::Debug for JobQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobQueue")
            .field("pool", &"<SqlitePool>")
            .field("handlers", &"<HashMap>")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl JobQueue {
    /// Initialize the queue database and schema.
    pub async fn init(db_path: impl AsRef<Path>) -> Result<Self, JobError> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await?;
        sqlx::query("PRAGMA journal_mode = WAL;")
            .execute(&pool)
            .await?;
        Self::init_schema(&pool).await?;

        Ok(Self {
            pool,
            handlers: Arc::new(RwLock::new(HashMap::new())),
            timeout: Arc::new(RwLock::new(Duration::from_secs(120 * 60))),
        })
    }

    /// Set the default timeout for subsequent manual or scheduled runs.
    pub async fn set_default_timeout(&self, timeout: Duration) {
        *self.timeout.write().await = timeout;
    }

    /// Register or update a job definition and in-process handler.
    pub async fn register(&self, job: Job) -> Result<(), JobError> {
        let schedule = job.schedule.map(DailySchedule::as_hhmm);
        let next_run_at = job.schedule.map(|s| s.next_after(Utc::now()));

        sqlx::query(
            "INSERT INTO jobs \
             (id, priority, schedule, yield_on_user_activity, next_run_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
               priority = excluded.priority, \
               schedule = excluded.schedule, \
               yield_on_user_activity = excluded.yield_on_user_activity, \
               next_run_at = excluded.next_run_at, \
               updated_at = excluded.updated_at",
        )
        .bind(&job.id)
        .bind(job.priority as i16)
        .bind(schedule)
        .bind(job.yield_on_user_activity)
        .bind(next_run_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        self.handlers.write().await.insert(job.id, job.handler);
        Ok(())
    }

    /// Execute a registered job immediately.
    pub async fn run_now(&self, job_id: &str) -> Result<JobRunSummary, JobError> {
        let handler = self
            .handlers
            .read()
            .await
            .get(job_id)
            .cloned()
            .ok_or_else(|| JobError::JobNotRegistered(job_id.to_string()))?;

        let running: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job_runs WHERE job_id = ? AND status = ?")
                .bind(job_id)
                .bind(JobRunStatus::Running.as_str())
                .fetch_one(&self.pool)
                .await?;
        if running > 0 {
            return Err(JobError::JobAlreadyRunning(job_id.to_string()));
        }

        let started_at = Utc::now();
        let run_id: i64 = sqlx::query_scalar(
            "INSERT INTO job_runs (job_id, status, started_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(job_id)
        .bind(JobRunStatus::Running.as_str())
        .bind(started_at)
        .fetch_one(&self.pool)
        .await?;

        let timeout = *self.timeout.read().await;
        let result =
            tokio::time::timeout(timeout, handler(JobContext::new(job_id.to_string()))).await;
        let (status, error) = match result {
            Ok(Ok(())) => (JobRunStatus::Succeeded, None),
            Ok(Err(e)) => (JobRunStatus::Failed, Some(e.to_string())),
            Err(_) => (JobRunStatus::TimedOut, Some("job timed out".to_string())),
        };
        let finished_at = Utc::now();

        sqlx::query("UPDATE job_runs SET status = ?, finished_at = ?, error = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(finished_at)
            .bind(&error)
            .bind(run_id)
            .execute(&self.pool)
            .await?;

        Ok(JobRunSummary {
            run_id,
            job_id: job_id.to_string(),
            status,
            started_at,
            finished_at: Some(finished_at),
            error,
        })
    }

    /// Return status for a known job, including its most recent run.
    pub async fn status(&self, job_id: &str) -> Result<JobStatus, JobError> {
        let row = sqlx::query("SELECT id, priority, schedule, next_run_at FROM jobs WHERE id = ?")
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| JobError::JobNotRegistered(job_id.to_string()))?;

        let schedule: Option<String> = row.try_get("schedule")?;
        let schedule = schedule.as_deref().map(DailySchedule::parse).transpose()?;

        let last_run = self.last_run(job_id).await?;

        Ok(JobStatus {
            job_id: row.try_get("id")?,
            priority: JobPriority::from_i16(row.try_get::<i16, _>("priority")?).ok_or_else(
                || JobError::Handler(format!("unknown priority value for job {}", job_id)),
            )?,
            schedule,
            next_run_at: row.try_get("next_run_at")?,
            last_run,
        })
    }

    /// List all registered jobs with their current status.
    pub async fn list_jobs(&self) -> Result<Vec<JobStatus>, JobError> {
        let rows = sqlx::query("SELECT id, priority, schedule, next_run_at FROM jobs ORDER BY id")
            .fetch_all(&self.pool)
            .await?;

        let mut statuses = Vec::with_capacity(rows.len());
        for row in rows {
            let job_id: String = row.try_get("id")?;
            let schedule: Option<String> = row.try_get("schedule")?;
            let schedule = schedule.as_deref().map(DailySchedule::parse).transpose()?;
            let last_run = self.last_run(&job_id).await?;

            let job_id_clone = job_id.clone();
            statuses.push(JobStatus {
                job_id,
                priority: JobPriority::from_i16(row.try_get::<i16, _>("priority")?).ok_or_else(
                    || {
                        JobError::Handler(format!(
                            "unknown priority value for job {}",
                            job_id_clone
                        ))
                    },
                )?,
                schedule,
                next_run_at: row.try_get("next_run_at")?,
                last_run,
            });
        }
        Ok(statuses)
    }

    async fn last_run(&self, job_id: &str) -> Result<Option<JobRunSummary>, JobError> {
        let row = sqlx::query(
            "SELECT id, job_id, status, started_at, finished_at, error \
             FROM job_runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(JobRunSummary {
                run_id: row.try_get("id")?,
                job_id: row.try_get("job_id")?,
                status: JobRunStatus::from_str(row.try_get::<String, _>("status")?.as_str()),
                started_at: row.try_get("started_at")?,
                finished_at: row.try_get("finished_at")?,
                error: row.try_get("error")?,
            })
        })
        .transpose()
    }

    async fn init_schema(pool: &SqlitePool) -> Result<(), JobError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                priority INTEGER NOT NULL,
                schedule TEXT,
                yield_on_user_activity BOOLEAN NOT NULL,
                next_run_at TIMESTAMP,
                updated_at TIMESTAMP NOT NULL
            )",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS job_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                status TEXT NOT NULL,
                started_at TIMESTAMP NOT NULL,
                finished_at TIMESTAMP,
                error TEXT
            )",
        )
        .execute(pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_job_runs_job ON job_runs(job_id, started_at)")
            .execute(pool)
            .await?;
        Ok(())
    }
}
