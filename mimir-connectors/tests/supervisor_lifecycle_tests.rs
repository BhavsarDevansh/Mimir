//! Supervised connector lifecycle: restore, shutdown, backoff, circuit breaker, and panic recovery.

#![cfg(feature = "test-mock-connector")]
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use mimir_connectors::MockSyncRecorder;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};

mod common;
use common::*;

// ---------------------------------------------------------------------------
// Startup restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_spawns_only_active_connectors() {
    let (kg, _dir) = init_kg().await;
    let mut a = upsert(
        "active",
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    a.config_json = with_slug("active", json!({ "__ctype": ConnectorType::Email as i64 }));
    let mut p = upsert(
        "paused",
        ConnectorType::Email,
        "test",
        ConnectorStatus::Paused,
    );
    p.config_json = with_slug("paused", json!({ "__ctype": ConnectorType::Email as i64 }));
    let mut e = upsert(
        "errored",
        ConnectorType::Email,
        "test",
        ConnectorStatus::Error,
    );
    e.config_json = with_slug("errored", json!({ "__ctype": ConnectorType::Email as i64 }));
    let mut s = upsert(
        "setup",
        ConnectorType::Email,
        "test",
        ConnectorStatus::Setup,
    );
    s.config_json = with_slug("setup", json!({ "__ctype": ConnectorType::Email as i64 }));

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
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    // No "cursor" key => MockConnector returns new_cursor: None ("unchanged").
    input.config_json = with_slug(
        "nocursor",
        json!({ "__ctype": ConnectorType::Email as i64 }),
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
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "flaky",
        json!({ "__ctype": ConnectorType::Email as i64, "fail_first": 2, "cursor": "recovered" }),
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
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "doomed",
        json!({ "__ctype": ConnectorType::Email as i64, "always_fail": true }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    // The recorder counts every sync attempt, so `len() == 3` proves the
    // breaker opened after exactly three panic attempts (max_failures = 3).
    let recorder = Arc::new(MockSyncRecorder::default());
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), recording_registry(recorder.clone()), rx);
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
    assert_eq!(
        recorder.len(),
        3,
        "the breaker must open after exactly three panic attempts"
    );
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
        json!({
            "__ctype": ConnectorType::Photos as i64,
            "health": { "auth_expired": "mock auth rejection" },
        }),
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
                        && c.last_error.as_deref() == Some("mock auth rejection")
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
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "panic",
        json!({ "__ctype": ConnectorType::Email as i64, "panic_first": 1, "cursor": "p1" }),
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
// Circuit breaker via the panic path (T2 / #207)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn circuit_breaker_trips_on_repeated_panics() {
    let (kg, _dir) = init_kg().await;
    let mut input = upsert(
        "panic-doomed",
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    // `make_supervisor` uses max_failures = 3; three consecutive panics must
    // trip the breaker exactly like three ordinary failures.
    input.config_json = with_slug(
        "panic-doomed",
        json!({ "__ctype": ConnectorType::Email as i64, "panic_first": 3 }),
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
                .map(|c| c.status() == Some(ConnectorStatus::Error))
                .unwrap_or(false)
        },
        Duration::from_secs(5),
    )
    .await;

    let after = kg.get_connector(row.id).await.unwrap().unwrap();
    assert_eq!(after.status(), Some(ConnectorStatus::Error));
    assert!(
        after
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("panicked")),
        "last_error must record the panic, got {:?}",
        after.last_error
    );
    wait_for_async(
        || async { !supervisor.is_running(row.id).await },
        Duration::from_secs(2),
    )
    .await;
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
        ConnectorType::Email,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "push",
        json!({ "__ctype": ConnectorType::Email as i64, "mode": "push", "interval_ms": 3600000 }),
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
