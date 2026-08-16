//! Durable job queue for background tasks.
//!
//! Core types ([`Job`], [`JobPriority`], [`JobRunStatus`], [`DailySchedule`],
//! [`JobError`], [`JobContext`], [`JobRunSummary`], [`JobStatus`]) live here;
//! the SQLite-backed [`JobQueue`] implementation lives in `queue`.

mod queue;
mod resources;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use queue::JobQueue;
pub(crate) use resources::ResourceGuard;
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

    /// Lowercase wire representation of the priority class.
    ///
    /// The HTTP API (`mimir-api-types`) carries priorities as strings, so
    /// this is the single source of truth for the wire contract — independent
    /// of the derived `Debug` repr (issue #264).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Maintenance => "maintenance",
            Self::User => "user",
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
    /// Lowercase wire representation of the run status.
    ///
    /// Shared by the DB layer and the HTTP API (`mimir-api-types`), so the
    /// wire contract is independent of the derived `Debug` repr (issue #264).
    /// Note `TimedOut` maps to `"timed_out"` (underscored), matching the DB
    /// representation rather than the `Debug`-derived `"timedout"`.
    pub fn as_str(self) -> &'static str {
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
    ///
    /// The format is strict: the input must be exactly five characters with
    /// zero-padded two-digit hour and minute fields (e.g. `"02:30"`, not
    /// `"2:30"`). Non-zero-padded forms are rejected so user-authored
    /// `[[scheduler]]` strings stay deterministic (issue #162).
    pub fn parse(value: &str) -> Result<Self, JobError> {
        let bytes = value.as_bytes();
        let strict = bytes.len() == 5
            && bytes[2] == b':'
            && bytes[0..2].iter().all(u8::is_ascii_digit)
            && bytes[3..5].iter().all(u8::is_ascii_digit);
        if !strict {
            return Err(JobError::InvalidSchedule(value.to_string()));
        }
        let time = NaiveTime::parse_from_str(value, "%H:%M")
            .map_err(|_| JobError::InvalidSchedule(value.to_string()))?;
        Ok(Self::new(time))
    }

    /// Convert a naive local datetime to UTC, handling DST gaps and ambiguities.
    ///
    /// Shared with the CLI date filters (`mimir/src/kb.rs`) so user-authored
    /// local times are interpreted consistently (issue #168).
    pub fn naive_to_utc_local(naive: chrono::NaiveDateTime) -> DateTime<Utc> {
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
    cancellation: tokio_util::sync::CancellationToken,
}

impl JobContext {
    fn new(job_id: String, cancellation: tokio_util::sync::CancellationToken) -> Self {
        Self {
            job_id,
            cancellation,
        }
    }

    /// Identifier of the currently running job.
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    /// Cancellation token for the current run.
    ///
    /// Long-running handlers should `tokio::select!` on
    /// `ctx.cancellation_token().cancelled()` at checkpoint boundaries so
    /// they can persist state and exit cleanly when the daemon shuts down or
    /// the job is cancelled (issue #91).
    pub fn cancellation_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancellation.clone()
    }

    /// Returns `true` once a cancellation has been requested for this run.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

/// Best-effort per-job resource limits.
///
/// Enforcement is OS-specific and never fails the job: if a limit cannot be
/// applied (unsupported platform, missing permissions, no writable cgroup),
/// the queue logs at debug level and runs the job without it (issue #91).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobResourceLimits {
    /// Maximum number of CPUs the job may run on (Linux CPU affinity).
    pub cpu_cores: Option<u8>,
    /// Scheduling priority (`nice` value, -20..=19) for the job's thread.
    pub nice_level: Option<i8>,
    /// Best-effort memory cap in bytes (Linux cgroup v2 `memory.max`).
    pub memory_limit_bytes: Option<u64>,
}

/// Durable job definition plus its in-process handler.
pub struct Job {
    id: String,
    priority: JobPriority,
    schedule: Option<DailySchedule>,
    yield_on_user_activity: bool,
    handler: Arc<JobHandler>,
    limits: JobResourceLimits,
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
            limits: JobResourceLimits::default(),
        }
    }

    /// Attach best-effort resource limits to this job.
    pub fn with_resource_limits(mut self, limits: JobResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Resource limits configured for this job.
    pub fn resource_limits(&self) -> JobResourceLimits {
        self.limits
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
    #[error("job cancelled")]
    Cancelled,
    #[error("job timed out")]
    TimedOut,
}

impl JobError {
    /// Returns `true` if this error indicates the requested job is not registered.
    pub fn is_not_registered(&self) -> bool {
        matches!(self, Self::JobNotRegistered(_))
    }

    /// Returns `true` if this error indicates the job is already running.
    pub fn is_already_running(&self) -> bool {
        matches!(self, Self::JobAlreadyRunning(_))
    }
}
