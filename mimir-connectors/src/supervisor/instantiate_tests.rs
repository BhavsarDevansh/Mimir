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
        facts_accepted: 0,
        facts_dropped: 0,
        facts_staged: 0,
        last_sync_at: None,
        last_error: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// Build a supervisor whose Photos/local factory captures the injected
/// config, plus the capture handle, the temp dir (kept alive for the
/// SQLite file), and the shutdown sender (kept alive so the supervisor's
/// watch receiver stays open).
async fn capturing_supervisor() -> (
    ConnectorSupervisor,
    Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    tempfile::TempDir,
    watch::Sender<bool>,
) {
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
    let (tx, rx) = watch::channel(false);
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);
    (supervisor, captured, dir, tx)
}

/// `instantiate` must inject the persisted `sync_cursor` (alongside the
/// existing identity keys) so incremental connectors can seed their
/// in-memory cursor at construction (C1 / #195). A `None` cursor is
/// injected as JSON `null`.
#[tokio::test]
async fn instantiate_injects_cursor() {
    let (supervisor, captured, _dir, _tx) = capturing_supervisor().await;

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
    let (supervisor, captured, _dir, _tx) = capturing_supervisor().await;

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
    let (supervisor, captured, _dir, _tx) = capturing_supervisor().await;

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

/// `instantiate` must hand every connector factory the supervisor's own
/// knowledge graph (issue #386 review): the supervisor writes connector rows
/// and cursors through `self.kg`, so a second graph in the context could
/// split facts and connector provenance across two databases.
#[tokio::test]
async fn instantiate_injects_supervisor_knowledge_graph() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let captured = Arc::new(std::sync::Mutex::new(None::<Arc<KnowledgeGraph>>));
    let capture = Arc::clone(&captured);
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Photos,
            "local".to_string(),
            FnConnectorFactory::new(move |_config, ctx| {
                *capture.lock().unwrap() = ctx.knowledge_graph.clone();
                Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (tx, rx) = watch::channel(false);
    let supervisor = ConnectorSupervisor::new(
        Arc::new(registry),
        Arc::clone(&kg),
        SupervisorConfig::default(),
        rx,
    );

    supervisor
        .instantiate(&row_with_cursor(None), ConnectorType::Photos)
        .expect("instantiate succeeds");
    let injected = captured.lock().unwrap().take().expect("context captured");
    assert!(
        Arc::ptr_eq(&kg, &injected),
        "the factory context must carry the supervisor's own knowledge graph"
    );
    drop(tx);
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
        facts_accepted: 0,
        facts_dropped: 0,
        facts_staged: 0,
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

/// `resolved_mode` surfaces the mode the row's factory produces, with no
/// side effects (issue #397) — the value behind `ConnectorResponse.mode`.
#[tokio::test]
async fn resolved_mode_returns_the_factory_connectors_mode() {
    let (supervisor, _captured, _dir, _tx) = capturing_supervisor().await;
    match supervisor.resolved_mode(&row_with_cursor(None)) {
        Some(ConnectorMode::Polling { .. }) => {}
        other => panic!("the default mock must resolve to polling, got {other:?}"),
    }
}

#[tokio::test]
async fn resolved_mode_returns_push_for_a_push_factory() {
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Photos,
            "local".to_string(),
            FnConnectorFactory::new(|config, _ctx| {
                let mock = crate::MockConnector::from_config(config)?;
                Ok(Arc::new(mock) as Arc<dyn Connector>)
            }),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);
    let mut row = row_with_cursor(None);
    row.config_json = serde_json::json!({ "mode": "push" }).to_string();
    assert_eq!(
        supervisor.resolved_mode(&row),
        Some(ConnectorMode::Push),
        "a push-configured row resolves to push mode"
    );
}

#[tokio::test]
async fn resolved_mode_is_none_for_unknown_type_and_invalid_config() {
    let (supervisor, _captured, _dir, _tx) = capturing_supervisor().await;
    let mut row = row_with_cursor(None);
    row.connector_type_id = 999;
    assert_eq!(
        supervisor.resolved_mode(&row),
        None,
        "an unknown connector type must omit the mode"
    );
    let mut row = row_with_cursor(None);
    row.config_json = "not json".to_string();
    assert_eq!(
        supervisor.resolved_mode(&row),
        None,
        "an invalid config must omit the mode"
    );
}

/// Build an Email/imap row carrying the given `config_json` and durable state
/// for the email `resolved_mode` tests.
#[cfg(feature = "email")]
fn email_row(config_json: &str, durable_state: Option<&str>) -> ConnectorRow {
    ConnectorRow {
        id: 8,
        connector_type_id: ConnectorType::Email as i16,
        slug: "gmail-personal".to_string(),
        backend: "imap".to_string(),
        display_name: "Gmail".to_string(),
        config_json: config_json.to_string(),
        status_id: ConnectorStatus::Active as i16,
        auth_state_id: ConnectorAuthState::Authenticated as i16,
        sync_cursor: None,
        durable_state: durable_state.map(str::to_string),
        facts_accepted: 0,
        facts_dropped: 0,
        facts_staged: 0,
        last_sync_at: None,
        last_error: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[cfg(feature = "email")]
#[tokio::test]
async fn resolved_mode_omits_unprobed_auto_and_reads_persisted_capability() {
    // Issue #397 review: an `auto`-mode email connector must not be reported
    // as `push` before its IMAP capability probe completes. Once a previous
    // cycle persisted the probed capability in the durable state, the
    // config-only construction resolves the true mode (polling for an
    // IDLE-less server, push otherwise).
    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Email,
            "imap".to_string(),
            crate::email::EmailConnectorFactory::new(),
        )
        .unwrap();
    let (_tx, rx) = watch::channel(false);
    let supervisor =
        ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);

    let auto = r#"{"host":"imap.gmail.com","auth":{"kind":"app_password","username":"me@gmail.com"},"mode":"auto"}"#;
    assert_eq!(
        supervisor.resolved_mode(&email_row(auto, None)),
        None,
        "an unprobed auto connector must omit the mode, not claim push"
    );

    let durable_no_idle = serde_json::json!({
        "pending": {},
        "terminal": [],
        "tombstones": [],
        "supports_idle": false
    })
    .to_string();
    assert!(
        matches!(
            supervisor.resolved_mode(&email_row(auto, Some(&durable_no_idle))),
            Some(ConnectorMode::Polling { .. })
        ),
        "a persisted 'no IDLE' capability must resolve auto to polling"
    );

    let durable_idle = serde_json::json!({
        "pending": {},
        "terminal": [],
        "tombstones": [],
        "supports_idle": true
    })
    .to_string();
    assert_eq!(
        supervisor.resolved_mode(&email_row(auto, Some(&durable_idle))),
        Some(ConnectorMode::Push),
        "a persisted IDLE capability must resolve auto to push"
    );

    let poll = r#"{"host":"imap.gmail.com","auth":{"kind":"app_password","username":"me@gmail.com"},"mode":"poll"}"#;
    assert!(
        matches!(
            supervisor.resolved_mode(&email_row(poll, None)),
            Some(ConnectorMode::Polling { .. })
        ),
        "an explicit poll mode resolves without a probe"
    );
}
