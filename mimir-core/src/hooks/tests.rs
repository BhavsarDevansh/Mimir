//! Unit tests for the hooks engine (issue #386): queue policies, key scope,
//! debounce window, idle gating, cooldown, retry, and shutdown.

use super::*;
use crate::job_queue::JobQueue;
use crate::llm::MockLlmClient;
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;

/// Test handler that records payloads (as `String`) and returns scripted
/// outcomes. When `blocking` is set, it signals `started` and waits for
/// `release` before returning, so tests can observe a running instance.
struct TestHandler {
    calls: StdMutex<Vec<(String, u8)>>,
    outcomes: StdMutex<VecDeque<HookOutcome>>,
    started: Notify,
    release: Notify,
    blocking: bool,
}

impl TestHandler {
    fn new(outcomes: Vec<HookOutcome>) -> Arc<Self> {
        Arc::new(Self {
            calls: StdMutex::new(Vec::new()),
            outcomes: StdMutex::new(outcomes.into()),
            started: Notify::new(),
            release: Notify::new(),
            blocking: false,
        })
    }

    fn blocking(outcomes: Vec<HookOutcome>) -> Arc<Self> {
        let mut handler = Self::new(outcomes);
        Arc::get_mut(&mut handler).unwrap().blocking = true;
        handler
    }

    fn calls(&self) -> Vec<(String, u8)> {
        self.calls.lock().unwrap().clone()
    }

    fn release(&self) {
        self.release.notify_waiters();
    }
}

#[async_trait]
impl HookHandler for TestHandler {
    async fn run(&self, payload: Arc<dyn Any + Send + Sync>, ctx: HookContext) -> HookOutcome {
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        self.calls.lock().unwrap().push((text, ctx.attempt));
        if self.blocking {
            self.started.notify_waiters();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.release.notified()).await;
        }
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(HookOutcome::Success)
    }
}

async fn test_engine() -> (Arc<HookEngine>, tempfile::TempDir, watch::Receiver<bool>) {
    let temp = tempfile::tempdir().unwrap();
    let jq = Arc::new(JobQueue::init(temp.path().join("jobs.db")).await.unwrap());
    let llm = Arc::new(MockLlmClient::builder().build());
    let (engine, shutdown_rx) = HookEngine::new(jq, llm);
    (engine, temp, shutdown_rx)
}

fn turn_trigger(session_id: i64, text: &str) -> Trigger {
    Trigger::TurnCompleted {
        session_id,
        payload: Arc::new(text.to_string()),
    }
}

fn hook(id: &str, policy: QueuePolicy, gate: Gate, handler: Arc<TestHandler>) -> Hook {
    Hook {
        id: id.to_string(),
        trigger: TriggerKind::TurnCompleted,
        key_scope: KeyScope::PerKey,
        policy,
        gate,
        retry: RetryPolicy::default(),
        max_pending: None,
        merge: None,
        handler,
    }
}

async fn wait_for<F: Fn() -> bool>(cond: F) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "condition not met within 5 seconds"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_async<F, Fut>(cond: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond().await {
        assert!(
            Instant::now() < deadline,
            "condition not met within 5 seconds"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn advance_time(duration: Duration) {
    tokio::time::advance(duration).await;
}

#[tokio::test]
async fn multiple_policy_enqueues_every_trigger_fifo() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    for (session, text) in [(1, "a"), (1, "b"), (1, "c")] {
        let outcomes = engine.trigger(turn_trigger(session, text)).await;
        assert_eq!(
            outcomes,
            vec![TriggerOutcome {
                hook_id: "h".to_string(),
                status: TriggerStatus::Enqueued,
            }]
        );
    }
    assert_eq!(engine.pending_depth_for("h").await, 3);

    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for(|| handler.calls().len() == 3).await;
    assert_eq!(
        handler.calls(),
        vec![
            ("a".to_string(), 1),
            ("b".to_string(), 1),
            ("c".to_string(), 1),
        ]
    );
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn multiple_policy_rejects_triggers_over_pending_capacity() {
    // `Hook::max_pending` bounds the per-hook pending queue (issue #386
    // review): the producer sees `QueueFull` instead of unbounded in-memory
    // payload retention, and the queue keeps only the accepted instances.
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    let mut h = hook("h", QueuePolicy::Multiple, Gate::Ungated, handler.clone());
    h.max_pending = Some(2);
    engine.register(h).await.unwrap();

    assert_eq!(
        engine.trigger(turn_trigger(1, "a")).await[0].status,
        TriggerStatus::Enqueued
    );
    assert_eq!(
        engine.trigger(turn_trigger(2, "b")).await[0].status,
        TriggerStatus::Enqueued
    );
    assert_eq!(
        engine.trigger(turn_trigger(3, "c")).await[0].status,
        TriggerStatus::QueueFull,
        "over-capacity triggers must surface as QueueFull"
    );
    assert_eq!(engine.pending_depth_for("h").await, 2);
}

#[tokio::test]
async fn first_wins_drops_new_triggers_while_pending() {
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::SingularFirstWins,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    assert_eq!(
        engine.trigger(turn_trigger(1, "a")).await[0].status,
        TriggerStatus::Enqueued
    );
    assert_eq!(
        engine.trigger(turn_trigger(1, "b")).await[0].status,
        TriggerStatus::Dropped
    );
    assert_eq!(engine.pending_depth_for("h").await, 1);
}

#[tokio::test]
async fn first_wins_drops_new_triggers_while_running() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::blocking(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::SingularFirstWins,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for_async(|| async { engine.running_count().await == 1 }).await;

    assert_eq!(
        engine.trigger(turn_trigger(1, "b")).await[0].status,
        TriggerStatus::Dropped
    );
    handler.release();
    wait_for(|| handler.calls().len() == 1).await;
    assert_eq!(handler.calls(), vec![("a".to_string(), 1)]);

    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn last_wins_replaces_pending_and_moves_to_tail() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::SingularLastWins {
                debounce: Duration::ZERO,
            },
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    engine.trigger(turn_trigger(2, "x")).await;
    assert_eq!(
        engine.trigger(turn_trigger(1, "b")).await[0].status,
        TriggerStatus::Replaced
    );
    // Session 1's instance moved to the tail: FIFO order is now [x, b].
    assert_eq!(engine.pending_depth_for("h").await, 2);

    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for(|| handler.calls().len() == 2).await;
    assert_eq!(
        handler.calls(),
        vec![("x".to_string(), 1), ("b".to_string(), 1)]
    );
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn last_wins_merge_accumulates_payloads() {
    fn merge_strings(
        old: Arc<dyn Any + Send + Sync>,
        new: Arc<dyn Any + Send + Sync>,
    ) -> Arc<dyn Any + Send + Sync> {
        match (old.downcast::<String>(), new.clone().downcast::<String>()) {
            (Ok(mut old), Ok(new)) => {
                Arc::get_mut(&mut old)
                    .unwrap()
                    .push_str(&format!("+{}", new.as_str()));
                old
            }
            _ => new,
        }
    }

    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    let mut h = hook(
        "h",
        QueuePolicy::SingularLastWins {
            debounce: Duration::ZERO,
        },
        Gate::Ungated,
        handler.clone(),
    );
    h.merge = Some(merge_strings);
    engine.register(h).await.unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    engine.trigger(turn_trigger(1, "b")).await;
    engine.trigger(turn_trigger(1, "c")).await;
    assert_eq!(engine.pending_depth_for("h").await, 1);

    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for(|| handler.calls().len() == 1).await;
    assert_eq!(handler.calls(), vec![("a+b+c".to_string(), 1)]);
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn last_wins_running_instance_is_unaffected() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::blocking(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::SingularLastWins {
                debounce: Duration::ZERO,
            },
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for_async(|| async { engine.running_count().await == 1 }).await;

    // A trigger while running enqueues a fresh pending instance.
    assert_eq!(
        engine.trigger(turn_trigger(1, "b")).await[0].status,
        TriggerStatus::Enqueued
    );
    assert_eq!(engine.pending_depth_for("h").await, 1);

    handler.release();
    wait_for(|| handler.calls().len() == 2).await;
    assert_eq!(
        handler.calls(),
        vec![("a".to_string(), 1), ("b".to_string(), 1)]
    );
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn per_key_scope_allows_one_pending_per_key() {
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "per-key",
            QueuePolicy::SingularLastWins {
                debounce: Duration::ZERO,
            },
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    engine.trigger(turn_trigger(2, "x")).await;
    assert_eq!(engine.pending_depth_for("per-key").await, 2);

    // A global hook stays unique across keys.
    let mut global = hook(
        "global",
        QueuePolicy::SingularFirstWins,
        Gate::Ungated,
        handler.clone(),
    );
    global.key_scope = KeyScope::Global;
    engine.register(global).await.unwrap();
    assert_eq!(
        engine.trigger(turn_trigger(1, "a")).await[1].status,
        TriggerStatus::Enqueued
    );
    assert_eq!(
        engine.trigger(turn_trigger(2, "x")).await[1].status,
        TriggerStatus::Dropped
    );
    assert_eq!(engine.pending_depth_for("global").await, 1);
}

#[tokio::test]
async fn debounce_window_delays_dispatch_and_resets_on_trigger() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::SingularLastWins {
                debounce: Duration::from_millis(200),
            },
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    tokio::time::pause();
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    // Wait for the dispatch loop to observe the pending instance, then send
    // a trigger inside the window; it replaces the payload and resets the
    // window.
    wait_for_async(|| async { engine.pending_depth_for("h").await == 1 }).await;
    engine.trigger(turn_trigger(1, "b")).await;

    advance_time(Duration::from_millis(100)).await;
    assert!(
        handler.calls().is_empty(),
        "debounce window must delay dispatch"
    );

    // After the window elapses the latest payload dispatches exactly once.
    wait_for(|| handler.calls().len() == 1).await;
    assert_eq!(handler.calls(), vec![("b".to_string(), 1)]);
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn idle_gate_blocks_dispatch_while_llm_busy() {
    let temp = tempfile::tempdir().unwrap();
    let jq = Arc::new(JobQueue::init(temp.path().join("jobs.db")).await.unwrap());
    let llm = Arc::new(MockLlmClient::builder().in_flight_count(1).build());
    let (engine, shutdown_rx) = HookEngine::new(jq, llm);
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::IdleGated {
                cooldown: Duration::ZERO,
            },
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    tokio::time::pause();
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    // Give the dispatch loop a deterministic chance to observe the busy
    // LLM pool and leave the hook queued.
    advance_time(Duration::from_millis(300)).await;
    assert!(
        handler.calls().is_empty(),
        "idle-gated hook must not dispatch while the LLM pool is busy"
    );
    assert_eq!(engine.pending_depth_for("h").await, 1);

    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn idle_gate_dispatches_when_llm_idle() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::IdleGated {
                cooldown: Duration::ZERO,
            },
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for(|| handler.calls().len() == 1).await;
    assert_eq!(handler.calls(), vec![("a".to_string(), 1)]);
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn cooldown_resets_on_user_activity() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::IdleGated {
                cooldown: Duration::from_millis(300),
            },
            handler.clone(),
        ))
        .await
        .unwrap();

    // Seed user activity so the cooldown gates the first dispatch.
    engine.notify_user_activity();
    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    tokio::time::pause();
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    // Advance into the initial cooldown window, then reset it with user
    // activity.
    advance_time(Duration::from_millis(100)).await;
    engine.notify_user_activity();

    advance_time(Duration::from_millis(150)).await;
    assert!(
        handler.calls().is_empty(),
        "cooldown must block dispatch after user activity"
    );

    wait_for(|| handler.calls().len() == 1).await;
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn retryable_failure_reenqueues_with_backoff_and_attempts() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![
        HookOutcome::RetryableFailure,
        HookOutcome::RetryableFailure,
        HookOutcome::Success,
    ]);
    let mut h = hook("h", QueuePolicy::Multiple, Gate::Ungated, handler.clone());
    h.retry = RetryPolicy {
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    engine.register(h).await.unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    wait_for(|| handler.calls().len() == 3).await;
    assert_eq!(
        handler.calls(),
        vec![
            ("a".to_string(), 1),
            ("a".to_string(), 2),
            ("a".to_string(), 3),
        ]
    );
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn retry_budget_exhausted_drops_instance() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![
        HookOutcome::RetryableFailure,
        HookOutcome::RetryableFailure,
        HookOutcome::Success,
    ]);
    let mut h = hook("h", QueuePolicy::Multiple, Gate::Ungated, handler.clone());
    h.retry = RetryPolicy {
        max_attempts: 2,
        backoff: Duration::from_millis(10),
    };
    engine.register(h).await.unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    wait_for(|| handler.calls().len() == 2).await;
    assert_eq!(
        handler.calls().len(),
        2,
        "retry budget exhausted: no third attempt"
    );
    assert_eq!(engine.pending_depth_for("h").await, 0);
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn terminal_failure_drops_instance() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![HookOutcome::TerminalFailure]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    wait_for(|| handler.calls().len() == 1).await;
    assert_eq!(handler.calls().len(), 1);
    assert_eq!(engine.pending_depth_for("h").await, 0);
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn shutdown_exits_dispatch_loop() {
    let (engine, _temp, shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    engine.shutdown().await;
    let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(result.is_ok(), "dispatch loop must exit on shutdown");
}

#[tokio::test]
async fn shutdown_cancels_in_flight_run_and_keeps_pending_instances() {
    // `shutdown` cancels the running instance, awaits the dispatch loop's
    // exit (so the terminal `job_runs` status is written before teardown),
    // and must never start a new dispatch after the signal (issue #386
    // review).
    let temp = tempfile::tempdir().unwrap();
    let jq = Arc::new(JobQueue::init(temp.path().join("jobs.db")).await.unwrap());
    let llm = Arc::new(MockLlmClient::builder().build());
    let (engine, shutdown_rx) = HookEngine::new(jq.clone(), llm);
    let handler = TestHandler::blocking(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    engine.trigger(turn_trigger(2, "b")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });
    wait_for_async(|| async { engine.running_count().await == 1 }).await;

    engine.shutdown().await;
    let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(result.is_ok(), "dispatch loop must exit on shutdown");
    assert_eq!(
        handler.calls().len(),
        1,
        "only the in-flight instance may run before shutdown"
    );
    assert_eq!(
        engine.pending_depth_for("h").await,
        1,
        "pending instances must not dispatch after shutdown"
    );
    let status = jq.status("h").await.unwrap();
    let last_run = status
        .last_run
        .expect("the cancelled run must be finalised");
    assert_eq!(
        last_run.status,
        crate::job_queue::JobRunStatus::Cancelled,
        "shutdown must finalise the in-flight run before teardown"
    );
    assert!(
        last_run.finished_at.is_some(),
        "the in-flight run must carry a finished_at timestamp"
    );
}

#[tokio::test]
async fn timed_out_run_is_requeued_as_retryable_failure() {
    // A run that hits the durable queue's timeout is aborted mid-flight: the
    // instance must be requeued with backoff instead of dropped, so a
    // timed-out connector extraction is re-attempted (issue #386 review).
    let temp = tempfile::tempdir().unwrap();
    let jq = Arc::new(JobQueue::init(temp.path().join("jobs.db")).await.unwrap());
    jq.set_default_timeout(Duration::from_millis(100)).await;
    let llm = Arc::new(MockLlmClient::builder().build());
    let (engine, shutdown_rx) = HookEngine::new(jq, llm);
    let handler = TestHandler::blocking(vec![]);
    let mut h = hook("h", QueuePolicy::Multiple, Gate::Ungated, handler.clone());
    h.retry = RetryPolicy {
        max_attempts: 2,
        backoff: Duration::from_millis(10),
    };
    engine.register(h).await.unwrap();

    engine.trigger(turn_trigger(1, "a")).await;
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    // Attempt 1 times out and is requeued; attempt 2 times out and exhausts
    // the retry budget.
    wait_for(|| handler.calls().len() == 2).await;
    wait_for_async(|| async { engine.pending_depth_for("h").await == 0 }).await;
    assert_eq!(
        handler.calls(),
        vec![("a".to_string(), 1), ("a".to_string(), 2)],
        "a timed-out run must be retried"
    );
    engine.shutdown().await;
    handle.await.unwrap();
}

#[tokio::test]
async fn pending_depth_reports_total_across_hooks() {
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "a",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();
    engine
        .register(hook(
            "b",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    engine.trigger(turn_trigger(1, "x")).await;
    engine.trigger(turn_trigger(1, "y")).await;
    // Each trigger fires both hooks, so two triggers enqueue four instances.
    assert_eq!(engine.pending_depth().await, 4);
    assert_eq!(engine.pending_depth_for("a").await, 2);
    assert_eq!(engine.pending_depth_for("b").await, 2);
    assert_eq!(engine.pending_depth_for("missing").await, 0);
}

#[tokio::test]
async fn register_rejects_duplicate_hook_id() {
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();
    let result = engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await;
    assert!(matches!(result, Err(HookError::AlreadyRegistered(_))));
}

#[tokio::test]
async fn force_run_executes_handler_with_empty_payload() {
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::new(vec![HookOutcome::Success]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    let summary = engine.force_run("h").await.unwrap();
    assert_eq!(summary.status, crate::job_queue::JobRunStatus::Succeeded);
    assert_eq!(handler.calls(), vec![(String::new(), 1)]);
}

#[tokio::test]
async fn force_run_rejects_unknown_and_running_hooks() {
    let (engine, _temp, _shutdown_rx) = test_engine().await;
    let handler = TestHandler::blocking(vec![HookOutcome::Success]);
    engine
        .register(hook(
            "h",
            QueuePolicy::Multiple,
            Gate::Ungated,
            handler.clone(),
        ))
        .await
        .unwrap();

    assert!(matches!(
        engine.force_run("missing").await,
        Err(HookError::NotRegistered(_))
    ));

    // Start a force run in the background; a second force run must be
    // rejected while the first is in flight.
    let engine_clone = Arc::clone(&engine);
    let handle = tokio::spawn(async move { engine_clone.force_run("h").await });
    wait_for(|| handler.calls().len() == 1).await;
    assert!(matches!(
        engine.force_run("h").await,
        Err(HookError::AlreadyRunning(_))
    ));
    handler.release();
    handle.await.unwrap().unwrap();
}

#[test]
fn backoff_doubles_per_attempt_and_caps_at_max() {
    assert_eq!(
        backoff_for(Duration::from_secs(30), 1),
        Duration::from_secs(30)
    );
    assert_eq!(
        backoff_for(Duration::from_secs(30), 2),
        Duration::from_secs(60)
    );
    assert_eq!(
        backoff_for(Duration::from_secs(30), 8),
        Duration::from_secs(3600),
        "backoff saturates at MAX_BACKOFF"
    );
    assert_eq!(
        backoff_for(Duration::from_secs(30), u8::MAX),
        Duration::from_secs(3600),
        "the doubling must never overflow Duration"
    );
}
