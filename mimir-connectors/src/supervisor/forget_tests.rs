use super::*;

use std::sync::Arc;

use tokio::sync::watch;

use crate::FnConnectorFactory;
use crate::connector::{Connector, ConnectorError, HealthStatus, SyncOptions, SyncOutcome};
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::ConnectorAuthState;

use super::control_tests::wait_for_running;

/// Test wrapper that records `forget()` calls on an inner mock connector,
/// so tests can assert the supervisor actually invokes the connector-local
/// cleanup half of the cascade.
struct ForgetRecordingConnector {
    inner: crate::MockConnector,
    forget_calls: Arc<std::sync::atomic::AtomicU32>,
}

#[async_trait::async_trait]
impl Connector for ForgetRecordingConnector {
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
        self.inner.sync(options).await
    }
    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        self.inner.extract().await
    }
    async fn forget(&self) -> Result<(), ConnectorError> {
        self.forget_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.forget().await
    }
}

/// `forget` must stop the runner and invoke the connector's local
/// `forget()` on the live instance (the instance whose in-memory state —
/// e.g. the Photos watcher — is exactly what the cleanup must tear down),
/// not on a re-instantiated copy: the factory must be called exactly once
/// (at `start`), proving `forget` used the live instance.
#[tokio::test]
async fn forget_stops_runner_and_calls_connector_forget() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let forget_calls = Arc::new(AtomicU32::new(0));
    let calls = Arc::clone(&forget_calls);
    let creations = Arc::new(AtomicU32::new(0));
    let created = Arc::clone(&creations);
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                created.fetch_add(1, Ordering::SeqCst);
                let inner = crate::MockConnector::from_config(config)?;
                Ok(Arc::new(ForgetRecordingConnector {
                    inner,
                    forget_calls: Arc::clone(&calls),
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
            slug: "forget-live".to_string(),
            backend: "test".to_string(),
            display_name: "Forget Live".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    supervisor.start(row.id).await.unwrap();
    wait_for_running(&supervisor, row.id).await;

    supervisor.forget(row.id).await.unwrap();

    assert_eq!(forget_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        creations.load(Ordering::SeqCst),
        1,
        "forget must run on the live instance, not a re-instantiated one"
    );
    assert!(!supervisor.is_running(row.id).await);
}

/// `forget` on a connector that was never started must re-instantiate
/// from the row and still invoke the connector's local cleanup.
#[tokio::test]
async fn forget_reinstantiates_when_not_running() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&dir.path().join("kg.db"))
            .await
            .unwrap(),
    );
    let forget_calls = Arc::new(AtomicU32::new(0));
    let calls = Arc::clone(&forget_calls);
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "test".to_string(),
            FnConnectorFactory::new(move |config, _ctx| {
                let inner = crate::MockConnector::from_config(config)?;
                Ok(Arc::new(ForgetRecordingConnector {
                    inner,
                    forget_calls: Arc::clone(&calls),
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
            slug: "forget-cold".to_string(),
            backend: "test".to_string(),
            display_name: "Forget Cold".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    // Note: no start() call — the connector is in Setup with no runner.
    supervisor.forget(row.id).await.unwrap();

    assert_eq!(forget_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn forget_unknown_id_returns_not_found() {
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
    let err = supervisor.forget(9999).await.unwrap_err();
    assert!(matches!(
        err,
        SupervisorError::Knowledge(mimir_knowledge::KnowledgeError::ConnectorNotFound(9999))
    ));
}
