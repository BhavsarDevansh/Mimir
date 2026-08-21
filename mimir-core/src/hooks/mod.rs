//! Typed background-task hooks engine (issue #386).
//!
//! Hooks make learning deterministic: instead of the conversational LLM
//! deciding whether to call the `remember` tool, the daemon enqueues typed
//! hook instances on deterministic triggers (turn completed, connector item
//! staged, fact inserted) and a runner dispatches them through the durable
//! [`JobQueue`] under per-hook queue policies, key scopes, and execution
//! gates.
//!
//! The pending queue is in-memory; runs stay durable in [`JobQueue`]. A
//! daemon restart loses only pending instances — chat re-triggers on the next
//! turn, connector items are re-staged by the durable sync cursor, and
//! condensation re-triggers on the next fact write.

#![deny(unsafe_code)]

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::time::Instant;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{Mutex, Notify, RwLock, watch};
use tracing::{debug, info, warn};

use crate::job_queue::{Job, JobContext, JobError, JobPriority, JobQueue, JobRunSummary};
use crate::llm::LlmBackend;

/// Typed trigger events (v1 minimal enum; the #68 domain-event bus can
/// become the general trigger source later).
#[derive(Debug, Clone)]
pub enum Trigger {
    /// A non-incognito chat turn completed (assistant response persisted).
    TurnCompleted {
        session_id: i64,
        payload: Arc<dyn Any + Send + Sync>,
    },
    /// A connector item was staged and needs LLM extraction.
    ConnectorItemStaged {
        item_id: String,
        payload: Arc<dyn Any + Send + Sync>,
    },
    /// A fact was inserted / memory became dirty.
    FactInserted,
}

impl Trigger {
    fn kind(&self) -> TriggerKind {
        match self {
            Trigger::TurnCompleted { .. } => TriggerKind::TurnCompleted,
            Trigger::ConnectorItemStaged { .. } => TriggerKind::ConnectorItemStaged,
            Trigger::FactInserted => TriggerKind::FactInserted,
        }
    }

    /// Singularity key for `PerKey` scoped hooks.
    fn key(&self) -> Option<String> {
        match self {
            Trigger::TurnCompleted { session_id, .. } => Some(session_id.to_string()),
            Trigger::ConnectorItemStaged { item_id, .. } => Some(item_id.clone()),
            Trigger::FactInserted => None,
        }
    }

    fn into_payload(self) -> Arc<dyn Any + Send + Sync> {
        match self {
            Trigger::TurnCompleted { payload, .. } => payload,
            Trigger::ConnectorItemStaged { payload, .. } => payload,
            Trigger::FactInserted => Arc::new(()),
        }
    }
}

/// Which trigger kind a hook responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    TurnCompleted,
    ConnectorItemStaged,
    FactInserted,
}

/// Queue policy for a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePolicy {
    /// Every trigger enqueues; FIFO.
    Multiple,
    /// First instance stays; new triggers dropped while one is pending or
    /// running.
    SingularFirstWins,
    /// A pending instance is replaced with the latest payload and re-enqueued
    /// at the tail (true debounce); a running instance is unaffected.
    SingularLastWins { debounce: Duration },
}

/// Key scope for singularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyScope {
    /// One pending instance for the whole hook.
    Global,
    /// One pending instance per trigger key (e.g. per `session_id`).
    PerKey,
}

/// Execution gate for a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Wait for user-activity cooldown and LLM pool idle before dispatching,
    /// so background work never steals LLM capacity from interactive chat.
    IdleGated { cooldown: Duration },
    /// Dispatch immediately; LLM work routes through the shared
    /// [`LlmWorkerPool`](crate::llm::LlmWorkerPool) system queue.
    Ungated,
}

/// Retry policy for hook executions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum attempts per instance (1 = no retry).
    pub max_attempts: u8,
    /// Base backoff between attempts; doubles per failed attempt.
    pub backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff: Duration::ZERO,
        }
    }
}

/// Outcome of a hook execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// The instance completed; drop it.
    Success,
    /// The instance failed transiently; re-enqueue with backoff while the
    /// retry budget lasts.
    RetryableFailure,
    /// The instance failed permanently; drop it. The handler is responsible
    /// for recording any durable terminal state.
    TerminalFailure,
}

/// Context passed to hook handlers.
#[derive(Debug, Clone)]
pub struct HookContext {
    /// 1-based attempt number for this run (includes prior attempts).
    pub attempt: u8,
    /// Maximum attempts for this hook.
    pub max_attempts: u8,
    /// Cancellation token for the durable run.
    pub cancellation_token: tokio_util::sync::CancellationToken,
}

/// Merge a newer payload into an older pending payload for `LastWins`
/// accumulation. Returns the merged payload; falls back to `new` when the
/// payload types do not match.
pub type PayloadMerge = fn(
    old: Arc<dyn Any + Send + Sync>,
    new: Arc<dyn Any + Send + Sync>,
) -> Arc<dyn Any + Send + Sync>;

/// Handler executed when a hook instance is dispatched.
#[async_trait]
pub trait HookHandler: Send + Sync {
    async fn run(&self, payload: Arc<dyn Any + Send + Sync>, ctx: HookContext) -> HookOutcome;
}

/// A registered hook: trigger, queue policy, key scope, gate, retry, handler.
pub struct Hook {
    pub id: String,
    pub trigger: TriggerKind,
    pub key_scope: KeyScope,
    pub policy: QueuePolicy,
    pub gate: Gate,
    pub retry: RetryPolicy,
    /// Optional payload merge for `LastWins` accumulation (e.g. chat turns
    /// accumulated since the last hook run).
    pub merge: Option<PayloadMerge>,
    pub handler: Arc<dyn HookHandler>,
}

/// Result of a trigger for one matching hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerStatus {
    /// A new instance was enqueued.
    Enqueued,
    /// `FirstWins`: an instance is already pending or running for the key.
    Dropped,
    /// `LastWins`: a pending instance was replaced with the latest payload.
    Replaced,
}

/// Per-hook result of a [`HookEngine::trigger`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerOutcome {
    pub hook_id: String,
    pub status: TriggerStatus,
}

/// Errors emitted by the hooks engine.
#[derive(Debug, Error)]
pub enum HookError {
    #[error("hook already registered: {0}")]
    AlreadyRegistered(String),
    #[error("hook not registered: {0}")]
    NotRegistered(String),
    #[error("hook already running: {0}")]
    AlreadyRunning(String),
    #[error("job queue error: {0}")]
    JobQueue(#[from] JobError),
}

/// A pending hook instance.
#[derive(Debug, Clone)]
struct PendingInstance {
    key: Option<String>,
    payload: Arc<dyn Any + Send + Sync>,
    /// Last trigger time (debounce window for `LastWins`).
    last_trigger_at: Instant,
    /// Earliest dispatch time (retry backoff).
    not_before: Instant,
    /// Failed attempts so far.
    attempts: u8,
}

impl PendingInstance {
    fn new(key: Option<String>, payload: Arc<dyn Any + Send + Sync>, now: Instant) -> Self {
        Self {
            key,
            payload,
            last_trigger_at: now,
            not_before: now,
            attempts: 0,
        }
    }
}

/// A hook instance currently executing.
struct RunningInstance {
    key: Option<String>,
    payload: Arc<dyn Any + Send + Sync>,
    attempts: u8,
    max_attempts: u8,
    handler: Arc<dyn HookHandler>,
    outcome: Option<HookOutcome>,
}

struct EngineInner {
    job_queue: Arc<JobQueue>,
    llm: Arc<dyn LlmBackend>,
    /// Registered hooks in registration order (dispatch iterates in order).
    hooks: RwLock<Vec<(String, Arc<Hook>)>>,
    /// Pending instances per hook id.
    pending: Mutex<HashMap<String, VecDeque<PendingInstance>>>,
    /// Running instance per hook id (one run at a time per hook).
    running: Mutex<HashMap<String, RunningInstance>>,
    /// Last user-activity instant (cooldown for idle-gated hooks). A
    /// `std::sync::Mutex` because it is never held across an `await`.
    last_user_activity: StdMutex<Option<Instant>>,
    notify: Notify,
    shutdown_tx: watch::Sender<bool>,
}

/// Typed background-task hooks engine.
///
/// A single `tokio::spawn` dispatch loop drains pending instances through the
/// durable [`JobQueue`] under each hook's queue policy, key scope, debounce
/// window, execution gate, and retry policy.
#[derive(Clone)]
pub struct HookEngine {
    inner: Arc<EngineInner>,
}

impl std::fmt::Debug for HookEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `EngineInner` holds async locks and a `dyn LlmBackend` that are
        // not `Debug`; report a structural summary instead of recursing.
        f.debug_struct("HookEngine").finish_non_exhaustive()
    }
}

impl HookEngine {
    /// Create a new engine over a durable job queue and the shared LLM
    /// backend (used for idle gating).
    ///
    /// Returns the engine and a shutdown receiver that the caller should pass
    /// to [`Self::start`].
    pub fn new(
        job_queue: Arc<JobQueue>,
        llm: Arc<dyn LlmBackend>,
    ) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let engine = Arc::new(Self {
            inner: Arc::new(EngineInner {
                job_queue,
                llm,
                hooks: RwLock::new(Vec::new()),
                pending: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                last_user_activity: StdMutex::new(None),
                notify: Notify::new(),
                shutdown_tx,
            }),
        });
        (engine, shutdown_rx)
    }

    /// Register a hook and its durable [`JobQueue`] job.
    ///
    /// The registered job's handler executes the hook's currently running
    /// instance (set by the dispatch loop immediately before `run_now`), so
    /// per-instance payloads flow through the durable run without extending
    /// the [`JobQueue`] API.
    pub async fn register(&self, hook: Hook) -> Result<(), HookError> {
        {
            let hooks = self.inner.hooks.read().await;
            if hooks.iter().any(|(id, _)| id == &hook.id) {
                return Err(HookError::AlreadyRegistered(hook.id));
            }
        }
        // The handler closure holds a `Weak` so the engine and the queue do
        // not form a reference cycle (queue -> handler -> engine -> queue).
        let weak = Arc::downgrade(&self.inner);
        let hook_id = hook.id.clone();
        self.inner
            .job_queue
            .register(Job::new(
                hook_id.clone(),
                JobPriority::System,
                None,
                true,
                move |ctx: JobContext| {
                    let weak = weak.clone();
                    let hook_id = hook_id.clone();
                    Box::pin(async move {
                        match weak.upgrade() {
                            Some(inner) => inner.execute_running(&hook_id, ctx).await,
                            None => Err(JobError::Handler("hook engine dropped".to_string())),
                        }
                    })
                },
            ))
            .await?;
        self.inner
            .hooks
            .write()
            .await
            .push((hook.id.clone(), Arc::new(hook)));
        Ok(())
    }

    /// Fire a trigger: enqueue (or drop / replace) an instance for every
    /// registered hook that responds to the trigger's kind.
    pub async fn trigger(&self, trigger: Trigger) -> Vec<TriggerOutcome> {
        let kind = trigger.kind();
        let key = trigger.key();
        let payload = trigger.into_payload();
        let mut outcomes = Vec::new();
        {
            let hooks = self.inner.hooks.read().await;
            let mut pending = self.inner.pending.lock().await;
            let running = self.inner.running.lock().await;
            for (hook_id, hook) in hooks.iter() {
                if hook.trigger != kind {
                    continue;
                }
                let effective_key = match hook.key_scope {
                    KeyScope::Global => None,
                    KeyScope::PerKey => key.clone(),
                };
                let status = enqueue(
                    hook_id,
                    hook,
                    effective_key,
                    Arc::clone(&payload),
                    &mut pending,
                    &running,
                );
                outcomes.push(TriggerOutcome {
                    hook_id: hook_id.clone(),
                    status,
                });
            }
        }
        if !outcomes.is_empty() {
            self.inner.notify.notify_one();
        }
        outcomes
    }

    /// Record that user activity occurred, resetting the cooldown for
    /// idle-gated hooks.
    pub fn notify_user_activity(&self) {
        *self.inner.last_user_activity.lock().unwrap() = Some(Instant::now());
        self.inner.notify.notify_one();
    }

    /// Signal the dispatch loop to shut down gracefully and cancel the
    /// in-flight hook run.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown_tx.send(true);
        let running = self.inner.running.lock().await;
        for hook_id in running.keys() {
            self.inner.job_queue.cancel(hook_id);
        }
    }

    /// Force a hook to run immediately, bypassing debounce, cooldown, idle
    /// gates, and the pending queue.
    ///
    /// The handler runs with an empty `()` payload, so this is only
    /// meaningful for hooks whose handler does not depend on the trigger
    /// payload (e.g. `memory.condensation`). Returns the durable run
    /// summary, or an error when the hook is not registered or already
    /// running.
    pub async fn force_run(&self, hook_id: &str) -> Result<JobRunSummary, HookError> {
        let hook = {
            let hooks = self.inner.hooks.read().await;
            hooks
                .iter()
                .find(|(id, _)| id == hook_id)
                .map(|(_, h)| Arc::clone(h))
        };
        let Some(hook) = hook else {
            return Err(HookError::NotRegistered(hook_id.to_string()));
        };
        {
            let mut running = self.inner.running.lock().await;
            if running.contains_key(hook_id) {
                return Err(HookError::AlreadyRunning(hook_id.to_string()));
            }
            running.insert(
                hook_id.to_string(),
                RunningInstance {
                    key: None,
                    payload: Arc::new(()),
                    attempts: 0,
                    max_attempts: hook.retry.max_attempts,
                    handler: Arc::clone(&hook.handler),
                    outcome: None,
                },
            );
        }
        info!("hooks: force-running '{hook_id}'");
        let result = self.inner.job_queue.run_now(hook_id).await;
        {
            let mut running = self.inner.running.lock().await;
            running.remove(hook_id);
        }
        result.map_err(HookError::JobQueue)
    }

    /// Total pending instances across all hooks.
    pub async fn pending_depth(&self) -> usize {
        self.inner
            .pending
            .lock()
            .await
            .values()
            .map(VecDeque::len)
            .sum()
    }

    /// Pending instances for one hook.
    pub async fn pending_depth_for(&self, hook_id: &str) -> usize {
        self.inner
            .pending
            .lock()
            .await
            .get(hook_id)
            .map_or(0, VecDeque::len)
    }

    /// Number of hooks with a running instance.
    pub async fn running_count(&self) -> usize {
        self.inner.running.lock().await.len()
    }

    /// Start the dispatch loop.
    ///
    /// Runs until the shutdown watch channel fires.
    pub async fn start(self: Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) {
        loop {
            if let Some((hook_id, instance)) = self.inner.next_dispatchable().await {
                self.inner.dispatch(hook_id, instance).await;
                continue;
            }
            let deadline = self.inner.next_deadline().await;
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    info!("hooks: shutting down");
                    break;
                }
                _ = self.inner.notify.notified() => {
                    debug!("hooks: woken by trigger or user activity");
                }
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

/// Apply the queue policy to a trigger for one hook.
fn enqueue(
    hook_id: &str,
    hook: &Hook,
    key: Option<String>,
    payload: Arc<dyn Any + Send + Sync>,
    pending: &mut HashMap<String, VecDeque<PendingInstance>>,
    running: &HashMap<String, RunningInstance>,
) -> TriggerStatus {
    let now = Instant::now();
    let queue = pending.entry(hook_id.to_string()).or_default();
    match hook.policy {
        QueuePolicy::Multiple => {
            queue.push_back(PendingInstance::new(key, payload, now));
            TriggerStatus::Enqueued
        }
        QueuePolicy::SingularFirstWins => {
            let running_has = running.get(hook_id).is_some_and(|r| r.key == key);
            if running_has || queue.iter().any(|i| i.key == key) {
                TriggerStatus::Dropped
            } else {
                queue.push_back(PendingInstance::new(key, payload, now));
                TriggerStatus::Enqueued
            }
        }
        QueuePolicy::SingularLastWins { .. } => {
            if let Some(idx) = queue.iter().position(|i| i.key == key) {
                // Replace the pending instance with the latest payload and
                // re-enqueue it at the tail (true debounce).
                let mut instance = queue.remove(idx).expect("position index");
                instance.payload = match hook.merge {
                    Some(merge) => merge(instance.payload, payload),
                    None => payload,
                };
                instance.last_trigger_at = now;
                queue.push_back(instance);
                TriggerStatus::Replaced
            } else {
                // A running instance is unaffected; the new trigger enqueues
                // a fresh pending instance.
                queue.push_back(PendingInstance::new(key, payload, now));
                TriggerStatus::Enqueued
            }
        }
    }
}

impl EngineInner {
    /// Execute the currently running instance for `hook_id`.
    ///
    /// Called by the durable [`JobQueue`] handler registered for the hook.
    /// The dispatch loop sets the running instance immediately before
    /// `run_now`, so the payload is always the instance being dispatched.
    async fn execute_running(&self, hook_id: &str, ctx: JobContext) -> Result<(), JobError> {
        let (payload, attempt, max_attempts, handler) = {
            let running = self.running.lock().await;
            let instance = running.get(hook_id).ok_or_else(|| {
                JobError::Handler(format!("hook '{hook_id}' has no running instance"))
            })?;
            (
                Arc::clone(&instance.payload),
                instance.attempts.saturating_add(1),
                instance.max_attempts,
                Arc::clone(&instance.handler),
            )
        };
        let outcome = handler
            .run(
                payload,
                HookContext {
                    attempt,
                    max_attempts,
                    cancellation_token: ctx.cancellation_token(),
                },
            )
            .await;
        let mut running = self.running.lock().await;
        if let Some(instance) = running.get_mut(hook_id) {
            instance.outcome = Some(outcome);
        }
        Ok(())
    }

    /// Find the next dispatchable instance, if any.
    ///
    /// Iterates hooks in registration order; for each hook not currently
    /// running, scans its queue for the first instance whose debounce,
    /// cooldown, and retry-backoff windows have elapsed (and, for idle-gated
    /// hooks, whose LLM pool is idle).
    async fn next_dispatchable(&self) -> Option<(String, PendingInstance)> {
        // Idle-gated hooks share one gate: when the pool is busy none of them
        // can dispatch, so check once before scanning (and never hold engine
        // locks across an await).
        let llm_idle = self.llm_is_idle().await;
        let hooks = self.hooks.read().await;
        let mut pending = self.pending.lock().await;
        let running = self.running.lock().await;
        for (hook_id, hook) in hooks.iter() {
            if running.contains_key(hook_id) {
                continue;
            }
            let Some(queue) = pending.get_mut(hook_id) else {
                continue;
            };
            let Some(idx) = queue.iter().position(|i| self.time_eligible(i, hook)) else {
                continue;
            };
            if matches!(hook.gate, Gate::IdleGated { .. }) && !llm_idle {
                continue;
            }
            let instance = queue.remove(idx).expect("position index");
            return Some((hook_id.clone(), instance));
        }
        None
    }

    /// Whether an instance's debounce, cooldown, and retry-backoff windows
    /// have all elapsed.
    fn time_eligible(&self, instance: &PendingInstance, hook: &Hook) -> bool {
        let now = Instant::now();
        if instance.not_before > now {
            return false;
        }
        if let QueuePolicy::SingularLastWins { debounce } = hook.policy
            && instance.last_trigger_at + debounce > now
        {
            return false;
        }
        if let Gate::IdleGated { cooldown } = hook.gate
            && let Some(last) = *self.last_user_activity.lock().unwrap()
            && last + cooldown > now
        {
            return false;
        }
        true
    }

    /// Dispatch one instance: mark it running, execute it through the durable
    /// queue, and apply the retry policy to the outcome.
    async fn dispatch(&self, hook_id: String, mut instance: PendingInstance) {
        let hook = {
            let hooks = self.hooks.read().await;
            hooks
                .iter()
                .find(|(id, _)| id == &hook_id)
                .map(|(_, h)| Arc::clone(h))
        };
        let Some(hook) = hook else {
            warn!("hooks: dispatch for unregistered hook '{hook_id}'");
            return;
        };
        {
            let mut running = self.running.lock().await;
            if running.contains_key(&hook_id) {
                // A force run (or another dispatch) claimed this hook between
                // `next_dispatchable` and here; re-queue the instance.
                drop(running);
                self.pending
                    .lock()
                    .await
                    .entry(hook_id.clone())
                    .or_default()
                    .push_back(instance);
                self.notify.notify_one();
                return;
            }
            running.insert(
                hook_id.clone(),
                RunningInstance {
                    key: instance.key.clone(),
                    payload: Arc::clone(&instance.payload),
                    attempts: instance.attempts,
                    max_attempts: hook.retry.max_attempts,
                    handler: Arc::clone(&hook.handler),
                    outcome: None,
                },
            );
        }
        info!(
            "hooks: dispatching '{hook_id}' (attempt {})",
            instance.attempts.saturating_add(1)
        );
        let result = self.job_queue.run_now(&hook_id).await;
        let outcome = {
            let mut running = self.running.lock().await;
            running.remove(&hook_id).and_then(|r| r.outcome)
        };
        match (result, outcome) {
            (Err(JobError::Cancelled), _) => {
                debug!("hooks: '{hook_id}' run cancelled; dropping instance");
            }
            (Err(error), _) => {
                warn!("hooks: '{hook_id}' run failed: {error}");
                self.requeue_after_failure(&hook_id, &hook, &mut instance)
                    .await;
            }
            (Ok(_), Some(HookOutcome::Success)) => {
                debug!("hooks: '{hook_id}' succeeded");
            }
            (Ok(_), Some(HookOutcome::TerminalFailure)) => {
                warn!("hooks: '{hook_id}' terminal failure; dropping instance");
            }
            (Ok(_), Some(HookOutcome::RetryableFailure)) => {
                self.requeue_after_failure(&hook_id, &hook, &mut instance)
                    .await;
            }
            (Ok(_), None) => {
                warn!("hooks: '{hook_id}' handler recorded no outcome; dropping instance");
            }
        }
    }

    /// Re-enqueue a failed instance with exponential backoff while the retry
    /// budget lasts.
    async fn requeue_after_failure(
        &self,
        hook_id: &str,
        hook: &Hook,
        instance: &mut PendingInstance,
    ) {
        if instance.attempts.saturating_add(1) >= hook.retry.max_attempts {
            warn!("hooks: '{hook_id}' retry budget exhausted; dropping instance");
            return;
        }
        instance.attempts = instance.attempts.saturating_add(1);
        instance.not_before = Instant::now() + backoff_for(hook.retry.backoff, instance.attempts);
        self.pending
            .lock()
            .await
            .entry(hook_id.to_string())
            .or_default()
            .push_back(instance.clone());
        self.notify.notify_one();
    }

    /// Next instant at which some pending instance may become dispatchable.
    async fn next_deadline(&self) -> Instant {
        let now = Instant::now();
        let mut deadline = now + Duration::from_secs(60);
        {
            let hooks = self.hooks.read().await;
            let pending = self.pending.lock().await;
            let running = self.running.lock().await;
            for (hook_id, hook) in hooks.iter() {
                if running.contains_key(hook_id) {
                    continue;
                }
                let Some(queue) = pending.get(hook_id) else {
                    continue;
                };
                for instance in queue.iter() {
                    let mut d = instance.not_before;
                    if let QueuePolicy::SingularLastWins { debounce } = hook.policy {
                        d = d.max(instance.last_trigger_at + debounce);
                    }
                    if let Gate::IdleGated { cooldown } = hook.gate
                        && let Some(last) = *self.last_user_activity.lock().unwrap()
                    {
                        d = d.max(last + cooldown);
                    }
                    deadline = deadline.min(d);
                }
            }
        }
        // An idle-gated instance whose only blocker is the LLM pool must be
        // re-checked soon.
        if self.has_llm_blocked_instance().await {
            deadline = deadline.min(now + Duration::from_secs(2));
        }
        deadline
    }

    /// Whether an idle-gated hook has a pending instance that is
    /// time-eligible but blocked on the LLM pool.
    async fn has_llm_blocked_instance(&self) -> bool {
        let hooks = self.hooks.read().await;
        let pending = self.pending.lock().await;
        let running = self.running.lock().await;
        for (hook_id, hook) in hooks.iter() {
            if !matches!(hook.gate, Gate::IdleGated { .. }) || running.contains_key(hook_id) {
                continue;
            }
            let Some(queue) = pending.get(hook_id) else {
                continue;
            };
            if queue.iter().any(|i| self.time_eligible(i, hook)) {
                return true;
            }
        }
        false
    }

    /// Whether the LLM worker pool is completely idle (no queued or in-flight
    /// jobs), mirroring the background scheduler's gate.
    async fn llm_is_idle(&self) -> bool {
        let idle = self.llm.pool_is_idle().await;
        debug!("hooks: LLM pool idle={idle}");
        idle
    }
}

/// Exponential backoff after `attempts` failed attempts: `base * 2^(attempts-1)`.
fn backoff_for(base: Duration, attempts: u8) -> Duration {
    base * 2u32.saturating_pow(u32::from(attempts.saturating_sub(1)))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
