use super::*;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::connector::{ConnectorAction, ConnectorError};
use crate::{ActError, FnConnectorFactory};
use mimir_knowledge::models::connector::UpsertConnectorInput;

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
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            crate::MockConnectorFactory,
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
    (Arc::new(supervisor), kg, row.id, dir, tx)
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
