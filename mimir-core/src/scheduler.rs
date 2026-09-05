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

use tokio::sync::Mutex;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::job_queue::{JobError, JobQueue, JobRunSummary};
use crate::llm::LlmBackend;

/// Typed identifier for background jobs known to the daemon scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DaemonJob {
    KnowledgeOptimization,
}

impl DaemonJob {
    /// Persistent string ID used by the [`JobQueue`] database.
    pub fn job_id(self) -> &'static str {
        match self {
            DaemonJob::KnowledgeOptimization => "knowledge.optimization",
        }
    }

    /// Parse a persistent job ID back into a [`DaemonJob`] variant.
    pub fn from_job_id(id: &str) -> Option<Self> {
        match id {
            "knowledge.optimization" => Some(DaemonJob::KnowledgeOptimization),
            _ => None,
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
    /// Test-only signal: the dispatch loop has completed one gating check.
    #[cfg(test)]
    loop_checked: Notify,
    debounce: Duration,
    cooldown: Duration,
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
            #[cfg(test)]
            loop_checked: Notify::new(),
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
    pub async fn submit(&self, job: DaemonJob) -> SubmitStatus {
        let running = self.running.lock().await;
        if running.as_ref() == Some(&job) {
            drop(running);
            self.last_submit_time.store(
                chrono::Utc::now().timestamp_millis() as u64,
                Ordering::Relaxed,
            );
            self.job_notify.notify_one();
            debug!("scheduler: {:?} already running, debounce extended", job);
            return SubmitStatus::AlreadyPending;
        }
        drop(running);

        let mut pending = self.pending.lock().await;
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

    /// Signal the dispatch loop to shut down gracefully.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        // Cancel any in-flight job so the dispatch loop (which awaits
        // `run_now` inline) can observe shutdown promptly (issue #91).
        self.job_queue.cancel_all();
    }

    /// Start the dispatch loop.
    ///
    /// Runs until the shutdown watch channel fires.
    pub async fn start(self: Arc<Self>, mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
        let mut next_scheduled_check = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            #[cfg(test)]
            self.loop_checked.notify_waiters();
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
                let pending = self.pending.lock().await;
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
                        let mut running = self.running.lock().await;
                        *running = Some(job);
                    }
                    {
                        let mut pending = self.pending.lock().await;
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
                        let mut running = self.running.lock().await;
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
        let idle = self.llm.pool_is_idle().await;
        debug!("scheduler: LLM pool idle={idle}");
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
                            let job = DaemonJob::from_job_id(&status.job_id);
                            if job.is_none() {
                                warn!("scheduler: unknown scheduled job '{}'", status.job_id);
                            }
                            if let Some(job) = job {
                                info!("scheduler: scheduled job {:?} is due ({})", job, next_run);
                                let _ = self.submit(job).await;
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
    use crate::job_queue::{Job, JobContext, JobPriority, JobRunStatus};
    use crate::llm::MockLlmClient;
    use std::time::Duration;
    use tokio::sync::Notify;
    use tokio::time::timeout;

    /// Wait on the real clock for the scheduler's cooldown baseline to age
    /// far enough to enter the next test phase. The polling interval stays
    /// short so the assertion observes the scheduler's own timestamps.
    async fn wait_until_elapsed(started_at_ms: u64, minimum: Duration) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let elapsed_ms =
            || (chrono::Utc::now().timestamp_millis() as u64).saturating_sub(started_at_ms);
        while elapsed_ms() < minimum.as_millis() as u64 {
            assert!(
                std::time::Instant::now() < deadline,
                "cooldown baseline did not age within 5 seconds"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn test_job_queue() -> (Arc<JobQueue>, tempfile::TempDir, Arc<Notify>) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jobs.db");
        let jq = Arc::new(JobQueue::init(&path).await.unwrap());
        let job_started = Arc::new(Notify::new());
        let handler_started = Arc::clone(&job_started);

        let opt = Job::new(
            "knowledge.optimization",
            JobPriority::System,
            None,
            true,
            move |_ctx: JobContext| {
                let handler_started = Arc::clone(&handler_started);
                Box::pin(async move {
                    handler_started.notify_one();
                    Ok(())
                })
            },
        );
        jq.register(opt).await.unwrap();

        (jq, temp, job_started)
    }

    #[tokio::test]
    async fn test_submit_dedupes_pending() {
        let (jq, _temp, _job_started) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, _shutdown_rx) =
            BackgroundScheduler::new(jq, llm, Duration::from_secs(1), Duration::from_secs(1));

        assert_eq!(
            sched.submit(DaemonJob::KnowledgeOptimization).await,
            SubmitStatus::Queued
        );
        assert_eq!(
            sched.submit(DaemonJob::KnowledgeOptimization).await,
            SubmitStatus::AlreadyPending
        );

        let pending = sched.pending.lock().await;
        assert_eq!(pending.len(), 1);
        assert!(pending.contains(&DaemonJob::KnowledgeOptimization));
    }

    #[tokio::test]
    async fn test_submit_dedupes_running() {
        let (jq, _temp, _job_started) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, _shutdown_rx) =
            BackgroundScheduler::new(jq, llm, Duration::from_secs(1), Duration::from_secs(1));

        // Simulate a job already running.
        *sched.running.lock().await = Some(DaemonJob::KnowledgeOptimization);

        assert_eq!(
            sched.submit(DaemonJob::KnowledgeOptimization).await,
            SubmitStatus::AlreadyPending
        );

        // Job should NOT be added to pending because it is already running.
        let pending = sched.pending.lock().await;
        assert!(!pending.contains(&DaemonJob::KnowledgeOptimization));
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_after_debounce_and_cooldown() {
        let (jq, _temp, job_started) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, shutdown_rx) = BackgroundScheduler::new(
            jq,
            llm,
            Duration::from_millis(100),
            Duration::from_millis(100),
        );

        sched.submit(DaemonJob::KnowledgeOptimization).await;
        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        timeout(Duration::from_secs(5), job_started.notified())
            .await
            .expect("scheduler must dispatch after debounce and cooldown");
        // Job should have been dispatched and removed from pending.
        assert!(sched.pending.lock().await.is_empty());
        sched.shutdown();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("scheduler must stop after shutdown")
            .expect("scheduler task panicked");
    }

    #[tokio::test]
    async fn test_user_activity_resets_cooldown() {
        let (jq, _temp, _job_started) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, shutdown_rx) =
            BackgroundScheduler::new(jq, llm, Duration::ZERO, Duration::from_millis(200));

        // Seed initial user activity so cooldown is active from the start.
        sched.notify_user_activity();

        // Submit and start scheduler.
        sched.submit(DaemonJob::KnowledgeOptimization).await;
        let first_check = sched.loop_checked.notified();
        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        timeout(Duration::from_secs(5), first_check)
            .await
            .expect("scheduler must check the queued job before shutdown");
        wait_until_elapsed(
            sched.last_user_activity.load(Ordering::Relaxed),
            Duration::from_millis(10),
        )
        .await;

        // Job should still be pending because cooldown hasn't elapsed.
        let pending = sched.pending.lock().await;
        assert!(pending.contains(&DaemonJob::KnowledgeOptimization));
        drop(pending);

        let second_check = sched.loop_checked.notified();

        // Simulate user activity.
        sched.notify_user_activity();
        timeout(Duration::from_secs(5), second_check)
            .await
            .expect("scheduler must re-check after user activity");

        // Still pending because cooldown reset.
        let pending = sched.pending.lock().await;
        assert!(pending.contains(&DaemonJob::KnowledgeOptimization));

        sched.shutdown();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_llm_busy_blocks_dispatch() {
        let (jq, _temp, _job_started) = test_job_queue().await;
        let llm = Arc::new(MockLlmClient::builder().in_flight_count(1).build());
        let (sched, shutdown_rx) =
            BackgroundScheduler::new(jq, llm, Duration::ZERO, Duration::ZERO);

        sched.submit(DaemonJob::KnowledgeOptimization).await;
        let first_check = sched.loop_checked.notified();
        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        timeout(Duration::from_secs(5), first_check)
            .await
            .expect("scheduler must check the queued job");

        // LLM is "busy" so job should still be pending.
        let pending = sched.pending.lock().await;
        assert!(pending.contains(&DaemonJob::KnowledgeOptimization));

        sched.shutdown();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_shutdown_cancels_running_job() {
        let (jq, _temp, _job_started) = test_job_queue().await;

        let started = Arc::new(Notify::new());
        let handler_started = Arc::clone(&started);
        jq.register(Job::new(
            "knowledge.optimization",
            JobPriority::System,
            None,
            true,
            move |ctx: JobContext| {
                let handler_started = Arc::clone(&handler_started);
                Box::pin(async move {
                    handler_started.notify_one();
                    ctx.cancellation_token().cancelled().await;
                    Ok(())
                })
            },
        ))
        .await
        .unwrap();

        let llm = Arc::new(MockLlmClient::builder().build());
        let (sched, shutdown_rx) = BackgroundScheduler::new(
            Arc::clone(&jq),
            llm,
            Duration::from_millis(50),
            Duration::from_millis(50),
        );

        sched.submit(DaemonJob::KnowledgeOptimization).await;
        let sched_clone = Arc::clone(&sched);
        let handle = tokio::spawn(async move {
            sched_clone.start(shutdown_rx).await;
        });

        timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("scheduled job must start before shutdown");

        sched.shutdown();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("scheduler must stop after shutdown")
            .expect("scheduler task panicked");

        let status = jq.status("knowledge.optimization").await.unwrap();
        assert_eq!(
            status.last_run.as_ref().unwrap().status,
            JobRunStatus::Cancelled
        );
    }
}
