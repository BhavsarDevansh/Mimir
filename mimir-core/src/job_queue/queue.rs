//! SQLite-backed job queue: registration, scheduling, and execution.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::job_queue::{
    DailySchedule, Job, JobContext, JobError, JobHandler, JobPriority, JobResourceLimits,
    JobRunStatus, JobRunSummary, JobStatus, ResourceGuard,
};

/// A registered job definition plus its in-process handler.
#[derive(Clone)]
struct RegisteredJob {
    handler: Arc<JobHandler>,
    limits: JobResourceLimits,
}

#[derive(Clone)]
pub struct JobQueue {
    pool: SqlitePool,
    handlers: Arc<RwLock<HashMap<String, RegisteredJob>>>,
    running_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
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
    /// Lock the running-token registry, recovering from poisoning.
    fn running_tokens(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.running_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

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
            running_tokens: Arc::new(Mutex::new(HashMap::new())),
            timeout: Arc::new(RwLock::new(Duration::from_secs(120 * 60))),
        })
    }

    /// Set the default timeout for subsequent manual or scheduled runs.
    pub async fn set_default_timeout(&self, timeout: Duration) {
        *self.timeout.write().await = timeout;
    }

    /// Finalize a run row with its terminal status, finish time, and error.
    async fn finalize_run(
        &self,
        run_id: i64,
        status: JobRunStatus,
        error: Option<String>,
    ) -> Result<DateTime<Utc>, JobError> {
        let finished_at = Utc::now();
        sqlx::query("UPDATE job_runs SET status = ?, finished_at = ?, error = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(finished_at)
            .bind(&error)
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(finished_at)
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

        self.handlers.write().await.insert(
            job.id,
            RegisteredJob {
                handler: job.handler,
                limits: job.limits,
            },
        );
        Ok(())
    }

    /// Execute a registered job immediately.
    ///
    /// The handler runs on a fresh dedicated thread with the job's best-effort
    /// resource limits applied, under the queue's timeout and a per-run
    /// cancellation token (see [`JobQueue::cancel`]).
    pub async fn run_now(&self, job_id: &str) -> Result<JobRunSummary, JobError> {
        let registered = self
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

        let token = CancellationToken::new();
        self.running_tokens()
            .insert(job_id.to_string(), token.clone());

        let timeout = *self.timeout.read().await;
        let job_id_owned = job_id.to_string();
        let handler = registered.handler;
        let limits = registered.limits;
        let token_inner = token.clone();
        let handle = tokio::runtime::Handle::current();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        // Run the handler on a fresh dedicated thread so per-job resource
        // limits (CPU affinity, nice, cgroup memory) apply for the whole run
        // and thread-local state (affinity, nice) is discarded when the
        // thread exits, never leaking into pooled threads (issue #91).
        let thread = match std::thread::Builder::new()
            .name(format!("mimir-job-{job_id}"))
            .spawn(move || {
                let guard = ResourceGuard::apply(limits, &job_id_owned);
                let ctx = JobContext::new(job_id_owned, token_inner.clone());
                let result = handle.block_on(async {
                    tokio::select! {
                        result = handler(ctx) => result,
                        _ = token_inner.cancelled() => Err(JobError::Cancelled),
                        _ = tokio::time::sleep(timeout) => Err(JobError::TimedOut),
                    }
                });
                // Restore resource limits before signalling completion so a
                // subsequent run can never observe (or clobber) the previous
                // run's thread/process state (issue #91).
                drop(guard);
                let _ = result_tx.send(result);
            }) {
            Ok(thread) => thread,
            Err(e) => {
                self.running_tokens().remove(job_id);
                self.finalize_run(
                    run_id,
                    JobRunStatus::Failed,
                    Some(format!("failed to spawn job thread: {e}")),
                )
                .await?;
                return Err(JobError::Io(e));
            }
        };

        let result = match result_rx.await {
            Ok(result) => result,
            // The sender is dropped during unwind, so a closed channel means
            // the job thread panicked.
            Err(_) => Err(JobError::Handler("job thread panicked".to_string())),
        };
        // The thread has already signalled completion; detach it so the OS
        // cleans up the exited thread without blocking the async task.
        drop(thread);
        let (status, error) = match result {
            Ok(()) if token.is_cancelled() => {
                (JobRunStatus::Cancelled, Some("job cancelled".to_string()))
            }
            Ok(()) => (JobRunStatus::Succeeded, None),
            Err(JobError::Cancelled) => {
                (JobRunStatus::Cancelled, Some("job cancelled".to_string()))
            }
            Err(JobError::TimedOut) => (JobRunStatus::TimedOut, Some("job timed out".to_string())),
            Err(e) => (JobRunStatus::Failed, Some(e.to_string())),
        };
        self.running_tokens().remove(job_id);
        let finished_at = self.finalize_run(run_id, status, error.clone()).await?;

        Ok(JobRunSummary {
            run_id,
            job_id: job_id.to_string(),
            status,
            started_at,
            finished_at: Some(finished_at),
            error,
        })
    }

    /// Request cancellation of a running job.
    ///
    /// Cooperative handlers observe the token via [`JobContext::is_cancelled`]
    /// and exit cleanly; non-cooperative handlers are dropped when the run
    /// future is cancelled. Returns `true` if a running job was found.
    pub fn cancel(&self, job_id: &str) -> bool {
        let token = self.running_tokens().get(job_id).cloned();
        match token {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Request cancellation of every running job (used during daemon shutdown).
    pub fn cancel_all(&self) {
        let tokens: Vec<CancellationToken> = self.running_tokens().values().cloned().collect();
        for token in tokens {
            token.cancel();
        }
    }

    /// Returns `true` if the job currently has an in-flight run.
    pub async fn is_running(&self, job_id: &str) -> bool {
        self.running_tokens().contains_key(job_id)
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
