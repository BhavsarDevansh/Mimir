//! F8/F9 behavioural tests (issues #185 / #186): the `ConnectorSupervisor`
//! supervised per-connector task lifecycle + manual sync triggering.
//!
//! These exercise spawn / restart-with-backoff / circuit-breaker /
//! auth-expired-pause / startup-restore / graceful-shutdown / cursor
//! persistence / trigger preemption against a real in-memory knowledge graph.
//! Since F13 (#190) the configurable, always-compiled `MockConnector` is the
//! single test connector: behaviour (mode, cadence, health/auth,
//! failure/panic injection, cursor) is read from `config_json`, and a
//! `MockSyncRecorder` observes `SyncOptions` for the F9 concurrency tests.
//! The previous private `TestConnector` has been removed (DRY).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use mimir_connectors::{
    Connector, ConnectorError, ConnectorRegistry, ConnectorSupervisor, FnConnectorFactory,
    MockConnector, MockConnectorFactory, MockSyncRecorder, SupervisorConfig, SyncOptions,
    TriggerError, TriggerOutcome,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};

// ---------------------------------------------------------------------------
// Knowledge-graph test harness
// ---------------------------------------------------------------------------

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn upsert(
    slug: &str,
    ctype: ConnectorType,
    backend: &str,
    status: ConnectorStatus,
) -> UpsertConnectorInput {
    UpsertConnectorInput {
        connector_type: ctype,
        slug: slug.to_string(),
        backend: backend.to_string(),
        display_name: slug.to_string(),
        config_json: "{}".to_string(),
        status: Some(status),
        auth_state: Some(ConnectorAuthState::Authenticated),
    }
}

fn fast_config() -> SupervisorConfig {
    SupervisorConfig {
        max_failures: 3,
        base_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

fn make_supervisor(
    kg: Arc<KnowledgeGraph>,
    registry: Arc<ConnectorRegistry>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> ConnectorSupervisor {
    ConnectorSupervisor::new(registry, kg, fast_config(), shutdown)
}

/// Poll an async `predicate` until it returns true or `timeout` elapses.
async fn wait_for_async<F, Fut>(predicate: F, timeout: Duration)
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("wait_for_async timed out after {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn with_slug(slug: &str, extra: serde_json::Value) -> String {
    let mut cfg = extra;
    if let serde_json::Value::Object(map) = &mut cfg {
        map.insert("__slug".to_string(), json!(slug));
    }
    serde_json::to_string(&cfg).unwrap()
}

// ---------------------------------------------------------------------------
// Registries (MockConnector-backed)
// ---------------------------------------------------------------------------

/// Build a registry whose `MockConnectorFactory` reads behaviour entirely from
/// `config_json` (including the row slug/type, smuggled in by the supervisor at
/// restore time). The mock is always compiled (F13 / #190), so this is the
/// single connector used by every lifecycle test.
fn test_registry() -> Arc<ConnectorRegistry> {
    let registry = ConnectorRegistry::new();
    for ctype in [
        ConnectorType::Gmail,
        ConnectorType::Calendar,
        ConnectorType::Photos,
    ] {
        registry
            .register(ctype, "test".to_string(), MockConnectorFactory)
            .unwrap();
    }
    Arc::new(registry)
}

/// A registry whose factory injects a shared [`MockSyncRecorder`] into every
/// constructed `MockConnector`, so the F9 trigger tests can observe the
/// `SyncOptions` each `sync()` receives and the peak concurrency. The recorder
/// is attached via [`MockConnector::with_recorder`] (not the config path).
fn recording_registry(recorder: Arc<MockSyncRecorder>) -> Arc<ConnectorRegistry> {
    let registry = ConnectorRegistry::new();
    for ctype in [
        ConnectorType::Gmail,
        ConnectorType::Calendar,
        ConnectorType::Photos,
    ] {
        let rec = recorder.clone();
        let factory = FnConnectorFactory::new(
            move |config| -> Result<Arc<dyn Connector>, ConnectorError> {
                Ok(
                    Arc::new(MockConnector::from_config(config)?.with_recorder(rec.clone()))
                        as Arc<dyn Connector>,
                )
            },
        );
        registry
            .register(ctype, "test".to_string(), factory)
            .unwrap();
    }
    Arc::new(registry)
}

// ---------------------------------------------------------------------------
// Startup restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_spawns_only_active_connectors() {
    let (kg, _dir) = init_kg().await;
    let mut a = upsert(
        "active",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    a.config_json = with_slug("active", json!({ "__ctype": ConnectorType::Gmail as i64 }));
    let mut p = upsert(
        "paused",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Paused,
    );
    p.config_json = with_slug("paused", json!({ "__ctype": ConnectorType::Gmail as i64 }));
    let mut e = upsert(
        "errored",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Error,
    );
    e.config_json = with_slug("errored", json!({ "__ctype": ConnectorType::Gmail as i64 }));
    let mut s = upsert(
        "setup",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Setup,
    );
    s.config_json = with_slug("setup", json!({ "__ctype": ConnectorType::Gmail as i64 }));

    kg.upsert_connector(a).await.unwrap();
    let paused_id = kg.upsert_connector(p).await.unwrap().id;
    let errored_id = kg.upsert_connector(e).await.unwrap().id;
    let setup_id = kg.upsert_connector(s).await.unwrap().id;
    let active_id = kg
        .get_connector_by_slug("active")
        .await
        .unwrap()
        .unwrap()
        .id;

    let kg = Arc::new(kg);
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    let spawned = supervisor.restore().await.unwrap();
    assert_eq!(spawned, 1, "only the Active connector is spawned");
    assert!(supervisor.is_running(active_id).await);
    assert!(!supervisor.is_running(paused_id).await);
    assert!(!supervisor.is_running(errored_id).await);
    assert!(!supervisor.is_running(setup_id).await);
    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Graceful shutdown + cursor persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_persists_cursor_and_exits_cleanly() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "curs",
        ConnectorType::Calendar,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "curs",
        json!({ "__ctype": ConnectorType::Calendar as i64, "cursor": "v1" }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();

    // Wait until at least one successful sync persists the cursor.
    wait_for_async(
        || async {
            kg.get_connector(row.id)
                .await
                .unwrap()
                .map(|c| c.sync_cursor.as_deref() == Some("v1"))
                .unwrap_or(false)
        },
        Duration::from_secs(3),
    )
    .await;

    tx.send(true).unwrap();
    supervisor.shutdown().await;
    assert!(!supervisor.is_running(row.id).await);

    let after = kg.get_connector(row.id).await.unwrap().unwrap();
    assert_eq!(after.sync_cursor.as_deref(), Some("v1"));
}

// ---------------------------------------------------------------------------
// None cursor (unchanged) must not wipe persisted progress token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn none_cursor_preserves_existing_sync_cursor() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "nocursor",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    // No "cursor" key => MockConnector returns new_cursor: None ("unchanged").
    input.config_json = with_slug(
        "nocursor",
        json!({ "__ctype": ConnectorType::Gmail as i64 }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    // Seed a pre-existing cursor so we can prove it survives a None-cursor cycle.
    // The seeded row already satisfies `sync_cursor`, `last_sync_at.is_some()`,
    // and `status == Active`, so capture the seeded timestamp and require the
    // supervisor to advance it — proving a None-cursor cycle actually ran
    // rather than the poll observing the pre-existing row state.
    let seeded = kg
        .update_sync_cursor(row.id, Some("seed-token"))
        .await
        .unwrap();
    let seeded_at = seeded.last_sync_at;

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();

    // Wait for the supervisor to run a None-cursor cycle: `touch_last_sync`
    // advances `last_sync_at` past the seeded value while the seeded cursor
    // stays intact (not wiped to NULL).
    wait_for_async(
        || async {
            kg.get_connector(row.id)
                .await
                .unwrap()
                .map(|c| {
                    c.sync_cursor.as_deref() == Some("seed-token")
                        && c.last_sync_at > seeded_at
                        && c.status() == Some(ConnectorStatus::Active)
                })
                .unwrap_or(false)
        },
        Duration::from_secs(3),
    )
    .await;

    tx.send(true).unwrap();
    supervisor.shutdown().await;

    let after = kg.get_connector(row.id).await.unwrap().unwrap();
    assert_eq!(after.sync_cursor.as_deref(), Some("seed-token"));
}

// ---------------------------------------------------------------------------
// Transient failures → backoff → success resets failure count
// ---------------------------------------------------------------------------

#[tokio::test]
async fn transient_failures_then_success_clears_last_error() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "flaky",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "flaky",
        json!({ "__ctype": ConnectorType::Gmail as i64, "fail_first": 2, "cursor": "recovered" }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();

    // `fail_first: 2` makes the first two syncs fail; the third succeeds and
    // persists the `recovered` cursor. Waiting for that cursor proves the
    // backoff-restart path actually ran — it cannot be satisfied by the
    // initial row state (no cursor, `last_error = NULL`).
    wait_for_async(
        || async {
            let c = kg.get_connector(row.id).await.unwrap();
            c.map(|c| {
                c.sync_cursor.as_deref() == Some("recovered")
                    && c.status() == Some(ConnectorStatus::Active)
                    && c.last_error.is_none()
            })
            .unwrap_or(false)
        },
        Duration::from_secs(5),
    )
    .await;

    tx.send(true).unwrap();
    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Circuit breaker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn circuit_breaker_sets_error_after_max_failures() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "doomed",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "doomed",
        json!({ "__ctype": ConnectorType::Gmail as i64, "always_fail": true }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();

    // After `max_failures` consecutive failures the breaker trips.
    wait_for_async(
        || async {
            kg.get_connector(row.id)
                .await
                .unwrap()
                .map(|c| c.status() == Some(ConnectorStatus::Error))
                .unwrap_or(false)
        },
        Duration::from_secs(5),
    )
    .await;

    let after = kg.get_connector(row.id).await.unwrap().unwrap();
    assert_eq!(after.status(), Some(ConnectorStatus::Error));
    assert!(after.last_error.is_some(), "last_error must be recorded");
    // The breaker status is set before the runner exits; poll for the task to
    // finish so the assertion does not race the runtime reaping it.
    wait_for_async(
        || async { !supervisor.is_running(row.id).await },
        Duration::from_secs(2),
    )
    .await;
    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Auth expired → paused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_expired_pauses_connector_and_exits() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "expired",
        ConnectorType::Photos,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "expired",
        json!({ "__ctype": ConnectorType::Photos as i64, "health": "auth_expired" }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();

    wait_for_async(
        || async {
            kg.get_connector(row.id)
                .await
                .unwrap()
                .map(|c| {
                    c.status() == Some(ConnectorStatus::Paused)
                        && c.auth_state() == Some(ConnectorAuthState::Expired)
                })
                .unwrap_or(false)
        },
        Duration::from_secs(5),
    )
    .await;

    // The DB state is set before the runner task returns, so poll for the
    // task to actually finish rather than asserting immediately (which would
    // race the runtime reaping the task).
    wait_for_async(
        || async { !supervisor.is_running(row.id).await },
        Duration::from_secs(2),
    )
    .await;
    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Panic recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_panic_is_recovered_then_succeeds() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "panic",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "panic",
        json!({ "__ctype": ConnectorType::Gmail as i64, "panic_first": 1, "cursor": "p1" }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();

    // First cycle panics (counted as a failure + backoff), the next succeeds.
    wait_for_async(
        || async {
            kg.get_connector(row.id)
                .await
                .unwrap()
                .map(|c| c.sync_cursor.as_deref() == Some("p1"))
                .unwrap_or(false)
        },
        Duration::from_secs(5),
    )
    .await;

    tx.send(true).unwrap();
    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Push mode: in-flight blocking sync is cancelled on shutdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_mode_blocking_sync_cancels_on_shutdown() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "push",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "push",
        json!({ "__ctype": ConnectorType::Gmail as i64, "mode": "push", "interval_ms": 3600000 }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();
    assert!(supervisor.is_running(row.id).await);

    tx.send(true).unwrap();
    supervisor.shutdown().await;
    assert!(!supervisor.is_running(row.id).await);
}

// ---------------------------------------------------------------------------
// Manual sync trigger (F9 / #186)
// ---------------------------------------------------------------------------

/// A long polling interval that a manual trigger must preempt (100 s). The
/// first automatic cycle still runs immediately on restore; subsequent cycles
/// only run when triggered.
const PREEMPT_INTERVAL_MS: u64 = 100_000;

/// Common harness for the trigger tests: one Active polling connector with a
/// recorder and a long interval, already restored.
async fn trigger_harness(
    slug: &str,
    recorder: Arc<MockSyncRecorder>,
    extra: serde_json::Value,
) -> (
    Arc<KnowledgeGraph>,
    ConnectorSupervisor,
    tokio::sync::watch::Sender<bool>,
    i32,
    tempfile::TempDir,
) {
    let (kg, dir) = init_kg().await;
    let mut input = upsert(slug, ConnectorType::Gmail, "test", ConnectorStatus::Active);
    let mut cfg = extra;
    if let serde_json::Value::Object(map) = &mut cfg {
        map.insert("__ctype".to_string(), json!(ConnectorType::Gmail as i64));
        map.insert("interval_ms".to_string(), json!(PREEMPT_INTERVAL_MS));
        map.insert("cursor".to_string(), json!("c"));
    }
    input.config_json = with_slug(slug, cfg);
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), recording_registry(recorder), rx);
    supervisor.restore().await.unwrap();
    (kg, supervisor, tx, row.id, dir)
}

#[tokio::test]
async fn trigger_sync_preempts_polling_interval() {
    let recorder = Arc::new(MockSyncRecorder::default());
    let (_kg, supervisor, _tx, id, _dir) =
        trigger_harness("preempt", recorder.clone(), json!({})).await;

    // The first automatic cycle runs immediately on restore.
    wait_for_async(|| async { !recorder.is_empty() }, Duration::from_secs(5)).await;

    // A manual trigger must run a second sync far sooner than the 100 s
    // polling interval.
    let outcome = supervisor
        .trigger_sync(id, SyncOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(outcome, TriggerOutcome::Ok { .. }),
        "triggered cycle should succeed: {outcome:?}"
    );
    wait_for_async(|| async { recorder.len() >= 2 }, Duration::from_secs(2)).await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_full_option_reaches_connector() {
    let recorder = Arc::new(MockSyncRecorder::default());
    let (_kg, supervisor, _tx, id, _dir) =
        trigger_harness("full", recorder.clone(), json!({})).await;

    wait_for_async(|| async { !recorder.is_empty() }, Duration::from_secs(5)).await;
    // The automatic cycle uses the default (incremental) options.
    assert_eq!(
        recorder.last().map(|o| o.full),
        Some(false),
        "automatic cycle should be incremental"
    );

    let outcome = supervisor
        .trigger_sync(
            id,
            SyncOptions {
                full: true,
                since: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(outcome, TriggerOutcome::Ok { .. }));

    // `--full` reaches the connector's `sync()`.
    wait_for_async(
        || async { recorder.last().map(|o| o.full).unwrap_or(false) },
        Duration::from_secs(2),
    )
    .await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_since_option_is_forwarded() {
    let recorder = Arc::new(MockSyncRecorder::default());
    let window = Duration::from_secs(60 * 60 * 24 * 7); // 7 days
    let (_kg, supervisor, _tx, id, _dir) =
        trigger_harness("since", recorder.clone(), json!({})).await;

    wait_for_async(|| async { !recorder.is_empty() }, Duration::from_secs(5)).await;

    supervisor
        .trigger_sync(
            id,
            SyncOptions {
                full: false,
                since: Some(window),
            },
        )
        .await
        .unwrap();

    wait_for_async(
        || async {
            recorder
                .last()
                .map(|o| o.since == Some(window))
                .unwrap_or(false)
        },
        Duration::from_secs(2),
    )
    .await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn concurrent_triggers_are_serialised_not_duplicated() {
    let recorder = Arc::new(MockSyncRecorder::default());
    // A per-sync delay so two concurrent triggers would overlap if they were
    // not serialised.
    let (_kg, supervisor, _tx, id, _dir) =
        trigger_harness("serial", recorder.clone(), json!({ "sync_delay_ms": 150 })).await;

    // Let the first automatic cycle finish before triggering.
    wait_for_async(|| async { !recorder.is_empty() }, Duration::from_secs(5)).await;
    let baseline = recorder.len();

    // Fire two triggers concurrently. The per-connector semaphore serialises
    // them: the second waits for the first cycle to complete, then runs its
    // own. Both must succeed and each must drive exactly one cycle.
    let (a, b) = tokio::join!(
        supervisor.trigger_sync(id, SyncOptions::default()),
        supervisor.trigger_sync(id, SyncOptions::default()),
    );
    assert!(
        a.is_ok() && b.is_ok(),
        "both triggers should complete: {a:?} {b:?}"
    );

    // Two triggered cycles ran (not coalesced into one).
    assert_eq!(
        recorder.len(),
        baseline + 2,
        "each trigger should drive its own cycle, not be duplicated/coalesced"
    );
    // No two `sync()` calls overlapped (serialised, not concurrent).
    assert_eq!(
        recorder.max_concurrent(),
        1,
        "sync() must never run concurrently on the same connector"
    );

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_on_non_running_connector_errors() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "paused",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Paused,
    );
    input.config_json = with_slug("paused", json!({ "__ctype": ConnectorType::Gmail as i64 }));
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    // Restore spawns nothing — the connector is Paused.
    supervisor.restore().await.unwrap();

    let err = supervisor
        .trigger_sync(row.id, SyncOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, TriggerError::NotRunning { id, status } if id == row.id && status == Some(ConnectorStatus::Paused)),
        "paused connector should report NotRunning: {err:?}"
    );

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_unknown_id_is_not_found() {
    let (kg, _dir) = init_kg().await;
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);

    let err = supervisor
        .trigger_sync(9999, SyncOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, TriggerError::NotFound(9999)),
        "unknown id should report NotFound: {err:?}"
    );

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_by_slug_resolves_and_runs() {
    let recorder = Arc::new(MockSyncRecorder::default());
    let (_kg, supervisor, _tx, _id, _dir) =
        trigger_harness("slug-target", recorder.clone(), json!({})).await;

    wait_for_async(|| async { !recorder.is_empty() }, Duration::from_secs(5)).await;

    let outcome = supervisor
        .trigger_sync_by_slug("slug-target", SyncOptions::default())
        .await
        .unwrap();
    assert!(matches!(outcome, TriggerOutcome::Ok { .. }));
    wait_for_async(|| async { recorder.len() >= 2 }, Duration::from_secs(2)).await;

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_unknown_slug_is_not_found() {
    let (kg, _dir) = init_kg().await;
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);

    let err = supervisor
        .trigger_sync_by_slug("no-such-slug", SyncOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, TriggerError::NotFoundSlug(ref slug) if slug == "no-such-slug"),
        "unknown slug should report NotFoundSlug: {err:?}"
    );

    supervisor.shutdown().await;
}

#[tokio::test]
async fn trigger_sync_on_push_connector_is_unsupported() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "pushy",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "pushy",
        json!({ "__ctype": ConnectorType::Gmail as i64, "mode": "push", "interval_ms": 3600000 }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    supervisor.restore().await.unwrap();
    assert!(supervisor.is_running(row.id).await);

    let err = supervisor
        .trigger_sync(row.id, SyncOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, TriggerError::PushUnsupported { id } if id == row.id),
        "push-mode connector should reject manual triggers: {err:?}"
    );

    tx.send(true).unwrap();
    supervisor.shutdown().await;
}
