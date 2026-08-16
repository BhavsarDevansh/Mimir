//! Single-connector stop semantics (A1 / #202).

#![cfg(feature = "test-mock-connector")]
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use mimir_knowledge::models::enums::{ConnectorStatus, ConnectorType};

mod common;
use common::*;

// ---------------------------------------------------------------------------
// Single-connector stop (A1 / #202)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stop_aborts_a_single_running_connector() {
    let (kg, _dir) = init_kg().await;
    let mut a = upsert(
        "stop-a",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    a.config_json = with_slug("stop-a", json!({ "__ctype": ConnectorType::Gmail as i64 }));
    let mut b = upsert(
        "stop-b",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    b.config_json = with_slug("stop-b", json!({ "__ctype": ConnectorType::Gmail as i64 }));
    let row_a = kg.upsert_connector(a).await.unwrap();
    let row_b = kg.upsert_connector(b).await.unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    assert_eq!(supervisor.restore().await.unwrap(), 2);
    assert!(supervisor.is_running(row_a.id).await);
    assert!(supervisor.is_running(row_b.id).await);

    // Stopping one runner must not affect the other.
    assert!(supervisor.stop(row_a.id).await);
    assert!(!supervisor.is_running(row_a.id).await);
    assert!(supervisor.is_running(row_b.id).await);

    // A second stop on the same id (already down) reports no action.
    assert!(!supervisor.stop(row_a.id).await);
    // An unknown id reports no action.
    assert!(!supervisor.stop(i32::MAX).await);

    supervisor.shutdown().await;
    assert!(!supervisor.is_running(row_b.id).await);
}

// ---------------------------------------------------------------------------
// Single-connector stop — already-finished runner (A1 / #202 review)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stop_returns_false_for_an_already_finished_runner() {
    let (kg, _dir) = init_kg().await;
    // `auth_fail` makes `authenticate()` fail at the handshake, so the runner
    // task exits immediately (it sets the row to `Paused` and returns) while
    // its `ConnectorHandle` stays in the supervisor map — the
    // "already-finished handle" path that `stop` must distinguish from a live
    // runner.
    let mut input = upsert(
        "stop-finished",
        ConnectorType::Gmail,
        "test",
        ConnectorStatus::Active,
    );
    input.config_json = with_slug(
        "stop-finished",
        json!({ "__ctype": ConnectorType::Gmail as i64, "auth_fail": true }),
    );
    let row = kg.upsert_connector(input).await.unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = make_supervisor(kg.clone(), test_registry(), rx);
    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the runner's auth handshake to fail and the task to finish. The
    // row flips to `Paused` and `is_running` reports false (finished task) even
    // though the stale handle is still present.
    wait_for_async(
        || async { !supervisor.is_running(row.id).await },
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        kg.get_connector(row.id).await.unwrap().unwrap().status(),
        Some(ConnectorStatus::Paused)
    );

    // Stopping an already-finished runner cleans up the stale handle but
    // reports that no *live* runner was stopped.
    assert!(!supervisor.stop(row.id).await);
    // A repeat stop reports no action (the handle is now gone).
    assert!(!supervisor.stop(row.id).await);
    assert!(!supervisor.stop(i32::MAX).await);

    supervisor.shutdown().await;
}
