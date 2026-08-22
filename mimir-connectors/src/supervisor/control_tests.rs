use super::*;

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::connector::{
    ConnectorAction, ConnectorError, ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
use crate::{ActError, FnConnectorFactory, MockSyncRecorder, TriggerError, TriggerOutcome};
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::ConnectorAuthState;

// -- start / pause / resume / act (Phase 3 A2 / #203) --
/// Build a supervisor + KG with the mock factory registered, and insert a
/// connector row in `Setup`/`Unauthenticated`. Returns the supervisor and
/// the new row id.
async fn supervisor_with_row(
    config_json: &str,
) -> (
    Arc<ConnectorSupervisor>,
    Arc<KnowledgeGraph>,
    i32,
    tempfile::TempDir,
    watch::Sender<bool>,
) {
    let (supervisor, kg, _recorder, id, dir, tx) =
        supervisor_with_row_and_recorder(config_json, Arc::new(MockSyncRecorder::default())).await;
    (supervisor, kg, id, dir, tx)
}

/// Like [`supervisor_with_row`], but the mock factory injects a shared
/// [`MockSyncRecorder`] into every constructed connector so tests can observe
/// sync concurrency (issue #266 regression tests).
async fn supervisor_with_row_and_recorder(
    config_json: &str,
    recorder: Arc<MockSyncRecorder>,
) -> (
    Arc<ConnectorSupervisor>,
    Arc<KnowledgeGraph>,
    Arc<MockSyncRecorder>,
    i32,
    tempfile::TempDir,
    watch::Sender<bool>,
) {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    let rec = Arc::clone(&recorder);
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                let connector =
                    crate::MockConnector::from_config(config)?.with_recorder(rec.clone());
                Ok(Arc::new(connector) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    // Keep the watch sender alive for the test's duration: dropping it
    // closes the channel, which the runner treats as a shutdown signal and
    // exits immediately.
    let (tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );
    let row = kg
        .create_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gmail-test".to_string(),
            backend: "test".to_string(),
            display_name: "Gmail Test".to_string(),
            config_json: config_json.to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();
    (Arc::new(supervisor), kg, recorder, row.id, dir, tx)
}

/// Poll `kg.get_connector(id).status()` until it equals `expected`, with
/// a generous deadline, so runner transitions are gated on an observable
/// condition rather than a fixed real-time sleep (which can be too short
/// on a loaded CI runner).
async fn wait_for_status(kg: &KnowledgeGraph, id: i32, expected: ConnectorStatus) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = kg.get_connector(id).await.unwrap().unwrap().status();
        if status == Some(expected) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for connector {id} to reach {expected:?} (last: {status:?})"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Poll until the runner for `id` is alive (spawned and past its auth
/// handshake), with a generous deadline.
pub(super) async fn wait_for_running(supervisor: &ConnectorSupervisor, id: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !supervisor.is_running(id).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for connector {id} runner to start"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Poll until the runner for `id` has exited naturally (auth-expiry /
/// breaker / panic), with a generous deadline.
async fn wait_for_runner_exit(supervisor: &ConnectorSupervisor, id: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while supervisor.is_running(id).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for connector {id} runner to exit"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn start_spawns_runner_and_flips_active() {
    let (supervisor, kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
    // Starts in Setup (create_connector with status None).
    let before = kg.get_connector(id).await.unwrap().unwrap();
    assert_eq!(before.status(), Some(ConnectorStatus::Setup));

    supervisor.start(id).await.unwrap();

    // Wait for the runner to complete its auth handshake and persist
    // Active.
    wait_for_status(&kg, id, ConnectorStatus::Active).await;
    assert!(supervisor.is_running(id).await);
    // Clean shutdown so the test does not leak a task.
    supervisor.stop(id).await;
}

#[tokio::test]
async fn start_unknown_id_returns_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    let (_tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );
    let err = supervisor.start(9999).await.unwrap_err();
    assert!(matches!(
        err,
        SupervisorError::Knowledge(mimir_knowledge::KnowledgeError::ConnectorNotFound(9999))
    ));
}

#[tokio::test]
async fn pause_stops_runner_and_flips_paused() {
    let (supervisor, kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
    supervisor.start(id).await.unwrap();
    wait_for_status(&kg, id, ConnectorStatus::Active).await;
    assert!(supervisor.is_running(id).await);

    supervisor.pause(id).await.unwrap();

    assert!(!supervisor.is_running(id).await);
    let after = kg.get_connector(id).await.unwrap().unwrap();
    assert_eq!(after.status(), Some(ConnectorStatus::Paused));
}

#[tokio::test]
async fn resume_respawns_after_pause() {
    let (supervisor, kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
    supervisor.start(id).await.unwrap();
    wait_for_status(&kg, id, ConnectorStatus::Active).await;
    supervisor.pause(id).await.unwrap();
    assert!(!supervisor.is_running(id).await);
    assert_eq!(
        kg.get_connector(id).await.unwrap().unwrap().status(),
        Some(ConnectorStatus::Paused)
    );

    supervisor.resume(id).await.unwrap();
    wait_for_status(&kg, id, ConnectorStatus::Active).await;

    assert!(supervisor.is_running(id).await);
    assert_eq!(
        kg.get_connector(id).await.unwrap().unwrap().status(),
        Some(ConnectorStatus::Active)
    );
    supervisor.stop(id).await;
}

#[tokio::test]
async fn trigger_sync_consults_live_mode_not_spawn_snapshot() {
    // Issue #397 review: the manual-sync push gate must consult the *live*
    // connector's mode — an `auto`-mode email connector resolves to polling
    // once its capability probe completes — rather than a mode snapshot
    // captured at spawn (which would reject manual sync for a connector that
    // actually polls).
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    let mode_override = Arc::new(StdMutex::new(None));
    let override_handle = Arc::clone(&mode_override);
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                let connector = crate::MockConnector::from_config(config)?
                    .with_mode_override(Arc::clone(&override_handle));
                Ok(Arc::new(connector) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor = Arc::new(ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    ));
    let row = kg
        .create_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gmail-test".to_string(),
            backend: "test".to_string(),
            display_name: "Gmail Test".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();
    supervisor.start(row.id).await.unwrap();
    wait_for_status(&kg, row.id, ConnectorStatus::Active).await;

    // The connector is polling; flipping the live mode to push must make
    // manual sync report PushUnsupported…
    *mode_override.lock().unwrap() = Some(ConnectorMode::Push);
    let err = supervisor
        .trigger_sync(row.id, SyncOptions::default())
        .await
        .expect_err("a live push-mode connector must reject manual sync");
    assert!(
        matches!(err, TriggerError::PushUnsupported { .. }),
        "expected PushUnsupported, got {err:?}"
    );

    // …and flipping it back to polling must restore manual sync.
    *mode_override.lock().unwrap() = None;
    match supervisor
        .trigger_sync(row.id, SyncOptions::default())
        .await
        .expect("a live polling-mode connector keeps manual sync")
    {
        TriggerOutcome::Ok { fetched, .. } => assert_eq!(fetched, 0),
        other => panic!("expected a successful trigger outcome, got {other:?}"),
    }
    supervisor.stop(row.id).await;
}

#[tokio::test]
async fn act_dispatches_to_live_connector() {
    let (supervisor, _kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
    supervisor.start(id).await.unwrap();
    wait_for_running(&supervisor, id).await;

    let result = supervisor
        .act(
            id,
            ConnectorAction {
                kind: "echo".to_string(),
                payload: serde_json::json!({
                    "native_id": "item-1",
                    "message": "ok",
                }),
            },
        )
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.native_id.as_deref(), Some("item-1"));
    assert_eq!(result.message.as_deref(), Some("ok"));
    supervisor.stop(id).await;
}

#[tokio::test]
async fn act_unsupported_kind_returns_error() {
    let (supervisor, _kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
    supervisor.start(id).await.unwrap();
    wait_for_running(&supervisor, id).await;

    let err = supervisor
        .act(
            id,
            ConnectorAction {
                kind: "bogus".to_string(),
                payload: serde_json::Value::Null,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ActError::Connector(ConnectorError::UnsupportedAction(_))
    ));
    supervisor.stop(id).await;
}

#[tokio::test]
async fn act_reinstantiates_when_not_running() {
    // A connector that was never started has no live handle; act must
    // re-instantiate from the row and still dispatch.
    let (supervisor, _kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
    // Note: no start() call — the connector is in Setup with no runner.
    let result = supervisor
        .act(
            id,
            ConnectorAction {
                kind: "echo".to_string(),
                payload: serde_json::json!({"native_id": "x"}),
            },
        )
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.native_id.as_deref(), Some("x"));
}

#[tokio::test]
async fn act_unknown_id_returns_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    let (_tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );
    let err = supervisor
        .act(
            9999,
            ConnectorAction {
                kind: "echo".to_string(),
                payload: serde_json::Value::Null,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ActError::NotFound(9999)));
}

/// `act` must not reuse a connector whose runner exited naturally
/// (auth-expiry / breaker / panic): the handle stays in the map with a
/// finished task, but its in-memory connector may hold stale credentials.
/// It must drop the stale handle and re-instantiate from the row, reading
/// fresh credentials from the secret store. A counting factory proves the
/// re-instantiation: one construction at `start`, then a second at `act`.
#[tokio::test]
async fn act_reinstantiates_after_runner_exits_naturally() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let creations = Arc::new(AtomicU32::new(0));
    let registry = ConnectorRegistry::new();
    let count = Arc::clone(&creations);
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                count.fetch_add(1, Ordering::SeqCst);
                // `config` carries `auth_fail: true` so the runner exits at
                // the auth handshake, plus `act_kind: "echo"` for the
                // write-back.
                let connector = crate::MockConnector::from_config(config)?;
                Ok(Arc::new(connector) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );

    let row = kg
        .create_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "stale-act".to_string(),
            backend: "test".to_string(),
            display_name: "Stale".to_string(),
            config_json: r#"{"act_kind":"echo","auth_fail":true}"#.to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    supervisor.start(row.id).await.unwrap();
    assert_eq!(
        creations.load(Ordering::SeqCst),
        1,
        "start instantiates once"
    );
    // The runner fails the auth handshake and exits; wait for it.
    wait_for_runner_exit(&supervisor, row.id).await;

    // A write-back after the runner exited: must re-instantiate (creation
    // #2), not reuse the stale in-memory connector.
    let result = supervisor
        .act(
            row.id,
            ConnectorAction {
                kind: "echo".to_string(),
                payload: serde_json::json!({"native_id": "fresh"}),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        creations.load(Ordering::SeqCst),
        2,
        "act must re-instantiate"
    );
    assert!(result.success);
    assert_eq!(result.native_id.as_deref(), Some("fresh"));
}

// -- per-connector lifecycle serialisation (issue #266) --

/// Regression test for issue #266: concurrent `start` / `resume` on the same
/// instance must be serialised per-connector so a re-spawn never leaks a
/// runner task. The per-connector lifecycle lock is held across the whole
/// stop → instantiate → spawn sequence, so a burst of concurrent starts
/// queues instead of racing; without it, two callers can both pass `stop`
/// before either `spawn_into`, and the second `spawn_into` overwrites the
/// first handle without aborting its task — leaving two live runners, one
/// tracked. The shared sync recorder proves the invariant: at most one
/// runner is ever mid-sync, and exactly one runner survives the burst.
///
/// A multi-thread runtime is required for the guard to be meaningful: on a
/// current-thread runtime the old race self-heals because task resumptions
/// are serialised, so the leak window never opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_start_resume_leaves_single_runner() {
    let (supervisor, _kg, recorder, id, _dir, _tx) = supervisor_with_row_and_recorder(
        r#"{"sync_delay_ms": 5000, "interval_ms": 100000}"#,
        Arc::new(MockSyncRecorder::default()),
    )
    .await;

    // Fire a burst of concurrent start/resume calls at the same instance.
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let supervisor = Arc::clone(&supervisor);
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            if i % 2 == 0 {
                supervisor.start(id).await
            } else {
                supervisor.resume(id).await
            }
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }

    // Exactly one live runner survives the burst.
    assert_eq!(supervisor.running_count().await, 1);
    assert!(supervisor.is_running(id).await);
    // Wait for the surviving runner's sync to be in flight, then give a
    // hypothetical leaked runner a grace window to start its own sync: a
    // leaked task would overlap the tracked runner's sync. The sync delay
    // (5 s) is far longer than the observation window (300 ms), so the
    // tracked runner is still mid-sync when the window closes even on a
    // loaded CI runner — a leak cannot hide behind a completed sync.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while recorder.max_concurrent() < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the surviving runner to sync"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        recorder.max_concurrent(),
        1,
        "a leaked runner would sync concurrently with the tracked runner"
    );
    supervisor.stop(id).await;
}

/// Regression test for issue #266: `pause` is a lifecycle mutation too, so it
/// must hold the same per-connector lifecycle lock as `start` / `resume`.
/// Without it, a concurrent `pause` + `start` can interleave as: pause's
/// `stop` (no-op) → start's spawn → pause's `Paused` write — leaving a
/// `Paused` row with a live runner that keeps syncing. Holding the lock
/// ourselves proves both methods queue on it, and the final state after the
/// pair runs is always consistent: `Paused` with no runner, or `Active` with
/// exactly one.
#[tokio::test]
async fn pause_and_start_share_the_per_connector_lifecycle_lock() {
    let (supervisor, kg, id, _dir, _tx) =
        supervisor_with_row(r#"{"sync_delay_ms": 500, "interval_ms": 100000}"#).await;
    supervisor.start(id).await.unwrap();
    wait_for_running(&supervisor, id).await;

    // Hold the per-connector lifecycle lock ourselves: both pause and start
    // must block on it (they share the same per-connector serialisation).
    let guard = supervisor.lifecycle_lock(id).await;
    let pause_supervisor = Arc::clone(&supervisor);
    let start_supervisor = Arc::clone(&supervisor);
    let pause = tokio::spawn(async move { pause_supervisor.pause(id).await });
    let start = tokio::spawn(async move { start_supervisor.start(id).await });

    // Neither may complete while the lock is held.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !pause.is_finished(),
        "pause must wait for the per-connector lifecycle lock"
    );
    assert!(
        !start.is_finished(),
        "start must wait for the per-connector lifecycle lock"
    );

    drop(guard);
    pause.await.unwrap().unwrap();
    start.await.unwrap().unwrap();

    // The serialised pair ends in a consistent state: a Paused row never has
    // a live runner, and an Active row always has exactly one.
    let status = kg.get_connector(id).await.unwrap().unwrap().status();
    let running = supervisor.is_running(id).await;
    match status {
        Some(ConnectorStatus::Paused) => {
            assert!(!running, "a Paused connector must not have a live runner")
        }
        Some(ConnectorStatus::Active) => {
            assert!(running, "an Active connector must have a live runner")
        }
        other => panic!("unexpected final status {other:?}"),
    }
    supervisor.stop(id).await;
}

/// Test wrapper whose `authenticate()` handshake blocks on a test-owned
/// gate, so tests can hold a runner mid-handshake and assert lifecycle
/// behaviour while it is there (issue #266).
struct GatedAuthConnector {
    inner: crate::MockConnector,
    entered: Arc<tokio::sync::Notify>,
    gate: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Connector for GatedAuthConnector {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn connector_type(&self) -> ConnectorType {
        self.inner.connector_type()
    }
    fn mode(&self) -> ConnectorMode {
        self.inner.mode()
    }
    fn config_schema(&self) -> serde_json::Value {
        self.inner.config_schema()
    }
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        self.entered.notify_one();
        self.gate.notified().await;
        Ok(ConnectorAuthState::Authenticated)
    }
    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        self.inner.health().await
    }
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        self.inner.sync(options).await
    }
    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        self.inner.extract().await
    }
    async fn forget(&self) -> Result<(), ConnectorError> {
        self.inner.forget().await
    }
}

/// Regression test for issue #266: `stop` must return promptly while the
/// runner is still in its auth handshake. The graceful stop signals the
/// runner and awaits its termination, and the handshake is preemptable by
/// the stop signal — otherwise a slow or hung handshake (an unreachable
/// IMAP/CalDAV server) would block `stop`, and with it `pause` / `DELETE` /
/// re-spawn, for the whole network timeout.
#[tokio::test]
async fn stop_preempts_an_in_flight_auth_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let entered_for_factory = Arc::clone(&entered);
    let gate_for_factory = Arc::clone(&gate);
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                let inner = crate::MockConnector::from_config(config)?;
                Ok(Arc::new(GatedAuthConnector {
                    inner,
                    entered: Arc::clone(&entered_for_factory),
                    gate: Arc::clone(&gate_for_factory),
                }) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );
    let row = kg
        .create_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gated-auth".to_string(),
            backend: "test".to_string(),
            display_name: "Gated Auth".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    // Spawn the runner; its auth handshake blocks on the gate.
    supervisor.start(row.id).await.unwrap();
    entered.notified().await;

    // `stop` must return while the handshake is still blocked, instead of
    // waiting for it to complete.
    let stopped = tokio::time::timeout(Duration::from_secs(1), supervisor.stop(row.id)).await;
    assert!(
        stopped.unwrap(),
        "stop must stop the live runner while it is mid-handshake"
    );
    assert_eq!(supervisor.running_count().await, 0);

    // Release the gate so the cancelled handshake future can drop cleanly.
    gate.notify_one();
}

/// Test wrapper whose `sync()` blocks on a test-owned gate, so a runner can
/// be held mid-cycle while `shutdown` runs (issue #266 regression test).
struct GatedSyncConnector {
    inner: crate::MockConnector,
    entered: Arc<tokio::sync::Notify>,
    gate: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

/// Sets a shared flag when dropped, proving the cycle future was fully
/// cancelled (its stack unwound) rather than merely having abort requested.
///
/// `Drop` sleeps briefly before setting the flag: task cancellation is
/// asynchronous, so without the sleep a shutdown that merely *requested*
/// cancellation (without awaiting the cycle) could race the flag and pass
/// the regression test for the wrong reason on a fast machine.
struct SyncDropFlag(Arc<std::sync::atomic::AtomicBool>);

impl Drop for SyncDropFlag {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(200));
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Connector for GatedSyncConnector {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn connector_type(&self) -> ConnectorType {
        self.inner.connector_type()
    }
    fn mode(&self) -> ConnectorMode {
        self.inner.mode()
    }
    fn config_schema(&self) -> serde_json::Value {
        self.inner.config_schema()
    }
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        self.inner.authenticate().await
    }
    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        self.inner.health().await
    }
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let _dropped = SyncDropFlag(Arc::clone(&self.dropped));
        self.entered.notify_one();
        self.gate.notified().await;
        self.inner.sync(options).await
    }
    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        self.inner.extract().await
    }
    async fn forget(&self) -> Result<(), ConnectorError> {
        self.inner.forget().await
    }
}

/// Regression test for issue #266: `ConnectorSupervisor::shutdown` must not
/// return while a runner's in-flight cycle task is still alive. `shutdown`
/// signals every runner first (the graceful path aborts and awaits the cycle
/// inside the runner) and, for the abort fallback, retains the cycle
/// `JoinHandle` in a registry so it is aborted and awaited after the runner
/// exits. Without that, an aborted runner drops the cycle handle un-awaited
/// and the cycle task can outlive `shutdown`, writing facts after teardown.
/// The gated sync holds a cycle in flight while `shutdown` runs; the drop
/// flag proves the cycle future was fully cancelled before `shutdown`
/// returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_awaits_an_in_flight_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered_for_factory = Arc::clone(&entered);
    let gate_for_factory = Arc::clone(&gate);
    let dropped_for_factory = Arc::clone(&dropped);
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                let inner = crate::MockConnector::from_config(config)?;
                Ok(Arc::new(GatedSyncConnector {
                    inner,
                    entered: Arc::clone(&entered_for_factory),
                    gate: Arc::clone(&gate_for_factory),
                    dropped: Arc::clone(&dropped_for_factory),
                }) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    // Keep the watch sender alive: dropping it closes the channel, which the
    // runner treats as a shutdown signal.
    let (_tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );
    let row = kg
        .create_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gated-sync".to_string(),
            backend: "test".to_string(),
            display_name: "Gated Sync".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    // Spawn the runner; its first cycle blocks on the gate mid-sync.
    supervisor.start(row.id).await.unwrap();
    entered.notified().await;

    // `shutdown` runs WITHOUT the daemon-wide watch being signalled first:
    // the supervisor itself must signal the runner and await the in-flight
    // cycle's termination.
    supervisor.shutdown().await;

    // The cycle future must be gone (fully cancelled) by the time `shutdown`
    // returns — a detached cycle would still be blocked on the gate here.
    assert!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        "shutdown must not return while the in-flight cycle task is alive"
    );
    // Release the gate so the (already cancelled) sync future can unwind
    // cleanly if it is ever resumed.
    gate.notify_one();
}
