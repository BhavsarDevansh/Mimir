//! Manual sync triggering: preemption, options forwarding, concurrency, and error cases.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use mimir_connectors::{
    ConnectorSupervisor, MockSyncRecorder, SyncOptions, TriggerError, TriggerOutcome,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::enums::{ConnectorStatus, ConnectorType};

mod common;
use common::*;

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
