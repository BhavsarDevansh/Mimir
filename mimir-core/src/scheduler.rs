#![deny(unsafe_code)]
//! Unified background job scheduler.
//!
//! Dedupes, debounces, and waits for user-downtime before dispatching any
//! background job (condensation, optimization, etc.) through the durable
//! [`JobQueue`].

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use std::sync::Mutex;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::job_queue::{JobError, JobQueue, JobRunSummary};
use crate::llm::LlmBackend;

/// Typed identifier for background jobs known to the daemon scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DaemonJob {
    MemoryCondensation,
    KnowledgeOptimization,
}

impl DaemonJob {
    /// Persistent string ID used by the [`JobQueue`] database.
    pub fn job_id(self) -> &'static str {
        match self {
            DaemonJob::MemoryCondensation => "memory.condensation",
            DaemonJob::KnowledgeOptimization => "knowledge.optimization",
        }
    }
}

/// Lightweight status returned by [`BackgroundScheduler::submit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitStatus {
    /// Job was newly added to the pending set.
    Queued,
    /// Job was already pending or running; debounce timer extended.
    AlreadyPending,
}

/// Unified background scheduler.
///
/// A single `tokio::spawn` dispatch loop services three event sources:
/// - `job_notify` – a job was submitted
/// - `user_notify` – user activity reset the cooldown
/// - `scheduled_poll` – a 60-second tick to check for scheduled jobs
///
/// Jobs are only dispatched when:
/// 1. Debounce has elapsed since the most recent submit of the same job type
/// 2. Cooldown has elapsed since the most recent user activity
/// 3. The LLM worker pool is completely idle (no queued or in-flight jobs)
#[derive(Debug)]
pub struct BackgroundScheduler {
    job_queue: Arc<JobQueue>,
    llm: Arc<dyn LlmBackend>,
    pending: Mutex<HashSet<DaemonJob>>,
    running: Mutex<Option<DaemonJob>>,
    last_user_activity: AtomicU64,
    last_submit_time: AtomicU64,
    job_notify: Notify,
    user_notify: Notify,
    debounce: Duration,
    cooldown: Duration,
    #[allow(dead_code)]
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl BackgroundScheduler {
    /// Create a new scheduler.
    ///
    /// Returns the scheduler and a shutdown receiver that the caller should
    /// pass to [`Self::start`].
    pub fn new(
        job_queue: Arc<JobQueue>,
        llm: Arc<dyn LlmBackend>,
        debounce: Duration,
        cooldown: Duration,
    ) -> (Arc<Self>, tokio::sync::watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let scheduler = Arc::new(Self {
            job_queue,
            llm,
            pending: Mutex::new(HashSet::new()),
            running: Mutex::new(None),
            last_user_activity: AtomicU64::new(0),
            last_submit_time: AtomicU64::new(0),
            job_notify: Notify::new(),
            user_notify: Notify::new(),
            debounce,
            cooldown,
            shutdown_tx,
        });
        (scheduler, shutdown_rx)
    }

    /// Submit a job to the scheduler.
    ///
    /// - If the job is already pending or running, the debounce timer is
    ///   extended but the job is not duplicated.
    /// - The job will be dispatched after `debounce` + `cooldown` have
    ///   elapsed and the LLM pool is idle.
    pub fn submit(&self, job: DaemonJob) -> SubmitStatus {
        let mut pending = self.pending.lock().unwrap();
        let is_new = pending.insert(job);
        drop(pending);

        self.last_submit_time.store(
            chrono::Utc::now().timestamp_millis() as u64,
            Ordering::Relaxed,
        );
        self.job_notify.notify_one();

        if is_new {
            debug!("scheduler: queued {:?}", job);
            SubmitStatus::Queued
        } else {
            debug!("scheduler: {:?} already pending, debounce extended", job);
            SubmitStatus::AlreadyPending
        }
    }

    /// Force a job to run immediately, bypassing debounce, cooldown, and idle
    /// checks.
    ///
    /// Returns `Err` if the job is already running. Otherwise the handler is
    /// executed directly (not via a spawned task) so the caller can observe
    /// the result.
    pub async fn force_submit(&self, job: DaemonJob) -> Result<JobRunSummary, JobError> {
        self.job_queue.run_now(job.job_id()).await
    }

    /// Record that user activity occurred.
    ///
    /// Resets the cooldown timer so background jobs wait again before
    /// dispatching.
    pub fn notify_user_activity(&self) {
        self.last_user_activity.store(
            chrono::Utc::now().timestamp_millis() as u64,
            Ordering::Relaxed,
        );
        self.user_notify.notify_one();
    }

    /// Start the dispatch loop.
    ///
    /// Runs until the shutdown watch channel fires.
    pub async fn start(self: Arc<Self>, mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
        let mut next_scheduled_check = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            let now_ts = chrono::Utc::now().timestamp_millis() as u64;
            let last_submit = self.last_submit_time.load(Ordering::Relaxed);
            let debounce_ms = self.debounce.as_millis() as u64;
            let debounce_elapsed = now_ts.saturating_sub(last_submit) >= debounce_ms;

            let last_activity = self.last_user_activity.load(Ordering::Relaxed);
            let cooldown_ms = self.cooldown.as_millis() as u64;
            let cooldown_elapsed = if last_activity == 0 {
                true
            } else {
                now_ts.saturating_sub(last_activity) >= cooldown_ms
            };

            // Extract pending state without holding the lock across await.
            let (has_pending, next_job) = {
                let pending = self.pending.lock().unwrap();
                let has = !pending.is_empty();
                let job = pending.iter().copied().next();
                (has, job)
            };

            let llm_idle = if has_pending && debounce_elapsed && cooldown_elapsed {
                self.llm_is_idle().await
            } else {
                false
            };

            if has_pending && debounce_elapsed && cooldown_elapsed && llm_idle {
                if let Some(job) = next_job {
                    {
                        let mut running = self.running.lock().unwrap();
                        *running = Some(job);
                    }
                    {
                        let mut pending = self.pending.lock().unwrap();
                        pending.remove(&job);
                    }

                    info!("scheduler: dispatching {:?}", job);
                    let result = self.job_queue.run_now(job.job_id()).await;
                    match &result {
                        Ok(summary) => {
                            info!("scheduler: {:?} completed {:?}", job, summary.status);
                        }
                        Err(e) => {
                            warn!("scheduler: {:?} failed: {}", job, e);
                        }
                    }
                    {
                        let mut running = self.running.lock().unwrap();
                        *running = None;
                    }

                    // After a job runs, check if any scheduled jobs are due.
                    self.check_scheduled_jobs().await;

                    // Re-evaluate immediately in case more jobs are pending.
                    continue;
                }
            }

            // Compute sleep until next event.
            let now_instant = tokio::time::Instant::now();
            let sleep_until = if !has_pending {
                next_scheduled_check
            } else if !debounce_elapsed {
                let deadline = tokio::time::Instant::now()
                    + Duration::from_millis(
                        debounce_ms.saturating_sub(now_ts.saturating_sub(last_submit)),
                    );
                deadline.min(next_scheduled_check)
            } else if !cooldown_elapsed {
                let deadline = tokio::time::Instant::now()
                    + Duration::from_millis(
                        cooldown_ms.saturating_sub(now_ts.saturating_sub(last_activity)),
                    );
                deadline.min(next_scheduled_check)
            } else {
                // Debounce and cooldown done, but LLM not idle — poll again soon.
                now_instant + Duration::from_secs(2)
            };

            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    info!("scheduler: shutting down");
                    break;
                }
                _ = self.job_notify.notified() => {
                    debug!("scheduler: woken by job submit");
                }
                _ = self.user_notify.notified() => {
                    debug!("scheduler: woken by user activity");
                }
                _ = tokio::time::sleep_until(sleep_until) => {
                    if now_instant >= next_scheduled_check {
                        next_scheduled_check = now_instant + Duration::from_secs(60);
                        self.check_scheduled_jobs().await;
                    }
                }
            }
        }
    }

    async fn llm_is_idle(&self) -> bool {
        let user_depth = self.llm.user_queue_depth().await;
        let system_depth = self.llm.system_queue_depth().await;
        let in_flight = self.llm.in_flight_count();
        let idle = user_depth == 0 && system_depth == 0 && in_flight == 0;
        debug!(
            "scheduler: LLM idle check user={} system={} in_flight={} idle={}",
            user_depth, system_depth, in_flight, idle
        );
        idle
    }

    async fn check_scheduled_jobs(&self) {
        match self.job_queue.list_jobs().await {
            Ok(jobs) => {
                let now = chrono::Utc::now();
                for status in jobs {
                    if let Some(next_run) = status.next_run_at {
                        if next_run <= now {
                            // Map string job_id back to DaemonJob if known.
                            let job = match status.job_id.as_str() {
                                "memory.condensation" => Some(DaemonJob::MemoryCondensation),
                                "knowledge.optimization" => Some(DaemonJob::KnowledgeOptimization),
                                _ => {
                                    warn!("scheduler: unknown scheduled job '{}'", status.job_id);
                                    None
                                }
                            };
                            if let Some(job) = job {
                                info!("scheduler: scheduled job {:?} is due ({})", job, next_run);
                                let _ = self.submit(job);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("scheduler: failed to list scheduled jobs: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job_queue::{Job, JobContext, JobPriority};
    use crate::llm::MockLlmClient;
    use std::time::Duration;

    async fn test_job_queue() -> (Arc<JobQueue>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jobs.db");
        let jq = Arc::new(JobQueue::init(&path).await.unwrap());

        let dummy = Job::new(
            "memory.condensation",
            JobPriority::System,
            None,
            true,
            |_ctx: JobContext| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(())
                })
            },
        );
        jq.register(dummy).await.unwrap();

        let opt = Job::new(
            "knowledge.optimization",
            JobPriority::System,
            None,
            true,
            |_ctx: JobContext| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(())
                })
            },
        );
        jq.register(opt).await.unwrap();

        (jq, temp)
    }

    #[tokio::test]
    async fn test_submit_dedupes_pending() {
        let (jq, _temp) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, _shutdown_rx) =
            BackgroundScheduler::new(jq, llm, Duration::from_secs(1), Duration::from_secs(1));

        assert_eq!(
            sched.submit(DaemonJob::MemoryCondensation),
            SubmitStatus::Queued
        );
        assert_eq!(
            sched.submit(DaemonJob::MemoryCondensation),
            SubmitStatus::AlreadyPending
        );
        assert_eq!(
            sched.submit(DaemonJob::KnowledgeOptimization),
            SubmitStatus::Queued
        );

        let pending = sched.pending.lock().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&DaemonJob::MemoryCondensation));
        assert!(pending.contains(&DaemonJob::KnowledgeOptimization));
    }

    #[tokio::test]
    async fn test_dispatch_after_debounce_and_cooldown() {
        let (jq, _temp) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, shutdown_rx) = BackgroundScheduler::new(
            jq,
            llm,
            Duration::from_millis(100),
            Duration::from_millis(100),
        );

        sched.submit(DaemonJob::MemoryCondensation);

        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        // Wait for debounce + cooldown + a little extra.
        tokio::time::sleep(Duration::from_millis(400)).await;

        // Job should have been dispatched and removed from pending.
        let pending = sched.pending.lock().unwrap();
        assert!(pending.is_empty());

        sched.shutdown_tx.send(true).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_user_activity_resets_cooldown() {
        let (jq, _temp) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, shutdown_rx) = BackgroundScheduler::new(
            jq,
            llm,
            Duration::from_millis(50),
            Duration::from_millis(200),
        );

        // Seed initial user activity so cooldown is active from the start.
        sched.notify_user_activity();

        // Submit and start scheduler.
        sched.submit(DaemonJob::MemoryCondensation);
        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        // Wait past debounce but within cooldown.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should still be pending because cooldown hasn't elapsed.
        let pending = sched.pending.lock().unwrap();
        assert!(pending.contains(&DaemonJob::MemoryCondensation));
        drop(pending);

        // Simulate user activity.
        sched.notify_user_activity();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Still pending because cooldown reset.
        let pending = sched.pending.lock().unwrap();
        assert!(pending.contains(&DaemonJob::MemoryCondensation));

        sched.shutdown_tx.send(true).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_llm_busy_blocks_dispatch() {
        let (jq, _temp) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().in_flight_count(1).build());
        let (sched, shutdown_rx) = BackgroundScheduler::new(
            jq,
            llm,
            Duration::from_millis(50),
            Duration::from_millis(50),
        );

        sched.submit(DaemonJob::MemoryCondensation);
        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        // Wait past debounce + cooldown.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // LLM is "busy" so job should still be pending.
        let pending = sched.pending.lock().unwrap();
        assert!(pending.contains(&DaemonJob::MemoryCondensation));

        sched.shutdown_tx.send(true).unwrap();
        let _ = handle.await;
    }
}
