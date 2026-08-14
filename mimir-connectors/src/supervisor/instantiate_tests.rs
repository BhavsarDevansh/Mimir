use super::*;

use crate::FnConnectorFactory;
use chrono::Utc;
use mimir_knowledge::models::connector::Connector as ConnectorRow;
use mimir_knowledge::models::enums::ConnectorAuthState;

fn row_with_cursor(cursor: Option<&str>) -> ConnectorRow {
    ConnectorRow {
        id: 7,
        connector_type_id: ConnectorType::Photos as i16,
        slug: "photos".to_string(),
        backend: "local".to_string(),
        display_name: "Photos".to_string(),
        config_json: "{}".to_string(),
        status_id: ConnectorStatus::Active as i16,
        auth_state_id: ConnectorAuthState::Authenticated as i16,
        sync_cursor: cursor.map(str::to_string),
        durable_state: None,
        last_sync_at: None,
        last_error: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// `instantiate` must inject the persisted `sync_cursor` (alongside the
/// existing identity keys) so incremental connectors can seed their
/// in-memory cursor at construction (C1 / #195). A `None` cursor is
/// injected as JSON `null`.
#[tokio::test]
async fn instantiate_injects_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let captured = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let capture = captured.clone();
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Photos,
            "local".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                *capture.lock().unwrap() = Some(config.clone());
                Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);

    let connector = supervisor.instantiate(&row_with_cursor(Some("v1")), ConnectorType::Photos);
    assert!(connector.is_ok());

    let config = captured.lock().unwrap().take().expect("config captured");
    let map = config.as_object().expect("config is an object");
    assert_eq!(map.get("__slug").and_then(|v| v.as_str()), Some("photos"));
    // Derive the expected discriminant from the enum so the assertion
    // stays correct if `ConnectorType` ever changes its repr.
    assert_eq!(
        map.get("__ctype").and_then(|v| v.as_i64()),
        Some(ConnectorType::Photos as i64)
    );
    assert_eq!(map.get("__instance_id").and_then(|v| v.as_i64()), Some(7));
    assert_eq!(
        map.get("__cursor").and_then(|v| v.as_str()),
        Some("v1"),
        "persisted cursor must be injected for incremental connectors"
    );
    assert!(
        map.get("__durable_state")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "absent durable state must be injected as JSON null, not omitted"
    );
}

/// `instantiate` must inject the persisted `durable_state` (issue #262) so
/// connectors can seed durable retry state at construction.
#[tokio::test]
async fn instantiate_injects_durable_state() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let captured = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let capture = captured.clone();
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Photos,
            "local".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                *capture.lock().unwrap() = Some(config.clone());
                Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);

    let mut row = row_with_cursor(None);
    row.durable_state = Some("ledger-v1".to_string());
    supervisor
        .instantiate(&row, ConnectorType::Photos)
        .expect("instantiate succeeds");
    let config = captured.lock().unwrap().take().expect("config captured");
    let map = config.as_object().expect("config is an object");
    assert_eq!(
        map.get("__durable_state").and_then(|v| v.as_str()),
        Some("ledger-v1"),
        "persisted durable state must be injected"
    );
}

#[tokio::test]
async fn instantiate_injects_null_cursor_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let captured = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let capture = captured.clone();
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Photos,
            "local".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                *capture.lock().unwrap() = Some(config.clone());
                Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);

    // Assert the result instead of discarding it, so a construction
    // regression surfaces directly rather than failing later on the
    // opaque `expect("config captured")`.
    supervisor
        .instantiate(&row_with_cursor(None), ConnectorType::Photos)
        .expect("instantiate succeeds");
    let config = captured.lock().unwrap().take().expect("config captured");
    let map = config.as_object().expect("config is an object");
    assert!(
        map.get("__cursor").map(|v| v.is_null()).unwrap_or(false),
        "absent cursor must be injected as JSON null, not omitted"
    );
}

#[tokio::test]
async fn with_secret_store_propagates_into_factory_context() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    // Capture whether the factory received a context carrying the store.
    let saw_store = Arc::new(std::sync::Mutex::new(false));
    let saw_store_cap = saw_store.clone();
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Calendar,
            "caldav".to_string(),
            FnConnectorFactory::new(move |_config, ctx| {
                *saw_store_cap.lock().unwrap() = ctx.secret_store.is_some();
                Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let store: Arc<dyn crate::secrets::SecretStore> = Arc::new(crate::InMemorySecretStore::new());
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx)
            .with_secret_store(store);

    let cal_row = ConnectorRow {
        id: 9,
        connector_type_id: ConnectorType::Calendar as i16,
        slug: "calendar-personal".to_string(),
        backend: "caldav".to_string(),
        display_name: "Calendar".to_string(),
        config_json: "{}".to_string(),
        status_id: ConnectorStatus::Active as i16,
        auth_state_id: ConnectorAuthState::Authenticated as i16,
        sync_cursor: None,
        durable_state: None,
        last_sync_at: None,
        last_error: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    supervisor
        .instantiate(&cal_row, ConnectorType::Calendar)
        .expect("instantiate succeeds");
    assert!(
        *saw_store.lock().unwrap(),
        "with_secret_store must thread the store into the factory context"
    );
}
