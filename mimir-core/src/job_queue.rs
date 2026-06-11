//! Durable async job queue shared by daemon subsystems.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use thiserror::Error;
use tokio::sync::RwLock;

/// Boxed future returned by a job handler.
pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

type JobHandler = dyn Fn(JobContext) -> JobFuture + Send + Sync + 'static;

/// Priority class for queued jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i16)]
pub enum JobPriority {
    /// Daemon maintenance that should never compete with active user work.
    System = 0,
    /// Connector sync and background upkeep.
    Maintenance = 1,
    /// Explicitly requested user jobs.
    User = 2,
}

impl JobPriority {
    fn from_i16(value: i16) -> Option<Self> {
        match value {
            0 => Some(Self::System),
            1 => Some(Self::Maintenance),
            2 => Some(Self::User),
            _ => None,
        }
    }
}

/// Lifecycle status for a recorded job run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobRunStatus {
    /// Job was started and has not yet been finalized.
    Running,
    /// Job completed successfully.
    Succeeded,
    /// Job returned an error.
    Failed,
    /// Job exceeded its configured timeout.
    TimedOut,
    /// Job was cancelled during daemon shutdown.
    Cancelled,
}

impl JobRunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "timed_out" => Self::TimedOut,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}

/// Daily local-time schedule represented as UTC for tests and daemon state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailySchedule {
    time: NaiveTime,
}

impl DailySchedule {
    /// Create a daily schedule at the given time.
    pub fn new(time: NaiveTime) -> Self {
        Self { time }
    }

    /// Parse an `HH:MM` schedule.
    pub fn parse(value: &str) -> Result<Self, JobError> {
        let time = NaiveTime::parse_from_str(value, "%H:%M")
            .map_err(|_| JobError::InvalidSchedule(value.to_string()))?;
        Ok(Self::new(time))
    }

    /// Convert a naive local datetime to UTC, handling DST gaps and ambiguities.
    fn naive_to_utc_local(naive: chrono::NaiveDateTime) -> DateTime<Utc> {
        match chrono::Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
            chrono::LocalResult::Ambiguous(earlier, _later) => earlier.with_timezone(&Utc),
            chrono::LocalResult::None => {
                // Spring-forward gap: advance by one hour and retry.
                let shifted = naive + chrono::Duration::hours(1);
                match chrono::Local.from_local_datetime(&shifted) {
                    chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
                    chrono::LocalResult::Ambiguous(earlier, _later) => earlier.with_timezone(&Utc),
                    chrono::LocalResult::None => {
                        // Fallback to UTC interpretation (should never reach here for 1-hour shifts).
                        chrono::Utc.from_local_datetime(&naive).single().unwrap()
                    }
                }
            }
        }
    }

    /// Return the next UTC instant strictly after `now`.
    pub fn next_after(self, now: DateTime<Utc>) -> DateTime<Utc> {
        let today = now.date_naive();
        let candidate = Self::naive_to_utc_local(today.and_time(self.time));
        if candidate > now {
            candidate
        } else {
            Self::naive_to_utc_local((today + chrono::Duration::days(1)).and_time(self.time))
        }
    }

    pub fn as_hhmm(self) -> String {
        self.time.format("%H:%M").to_string()
    }
}

/// Runtime context passed to job handlers.
#[derive(Debug, Clone)]
pub struct JobContext {
    job_id: String,
}

impl JobContext {
    fn new(job_id: String) -> Self {
        Self { job_id }
    }

    /// Identifier of the currently running job.
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
}

/// Durable job definition plus its in-process handler.
pub struct Job {
    id: String,
    priority: JobPriority,
    schedule: Option<DailySchedule>,
    yield_on_user_activity: bool,
    handler: Arc<JobHandler>,
}

impl Job {
    /// Create a job definition.
    pub fn new<F>(
        id: impl Into<String>,
        priority: JobPriority,
        schedule: Option<DailySchedule>,
        yield_on_user_activity: bool,
        handler: F,
    ) -> Self
    where
        F: Fn(JobContext) -> JobFuture + Send + Sync + 'static,
    {
        Self {
            id: id.into(),
            priority,
            schedule,
            yield_on_user_activity,
            handler: Arc::new(handler),
        }
    }
}

/// Summary of a single job run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRunSummary {
    pub run_id: i64,
    pub job_id: String,
    pub status: JobRunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Current status of a registered job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatus {
    pub job_id: String,
    pub priority: JobPriority,
    pub schedule: Option<DailySchedule>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run: Option<JobRunSummary>,
}

/// Errors emitted by the job queue.
#[derive(Debug, Error)]
pub enum JobError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("job not registered: {0}")]
    JobNotRegistered(String),
    #[error("invalid schedule: {0}")]
    InvalidSchedule(String),
    #[error("job failed: {0}")]
    Handler(String),
    #[error("job already running: {0}")]
    JobAlreadyRunning(String),
}

impl JobError {
    /// Returns  if this error indicates the requested job is not registered.
    pub fn is_not_registered(&self) -> bool {
        matches!(self, Self::JobNotRegistered(_))
    }

    /// Returns  if this error indicates the job is already running.
    pub fn is_already_running(&self) -> bool {
        matches!(self, Self::JobAlreadyRunning(_))
    }
}

/// Shared durable job queue.
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
