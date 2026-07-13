//! Always-compiled mock connector for testing.
//!
//! Placeholder scaffold. The full configurable in-memory test harness — which
//! emits canned normalized facts through the knowledge-graph pipeline — is
//! owned by Phase 3 issue **F13**. This minimal implementation satisfies the
//! full [`Connector`] trait so the always-compiled mock path stays valid
//! under every feature combination, including `--no-default-features`.

use std::time::Duration;

use chrono::Utc;

use crate::connector::{
    Connector, ConnectorError, ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

/// No-op mock connector used by integration tests.
///
/// F13 will expand this into a configurable harness; for now every async
/// method returns an empty-success outcome and the identity accessors report
/// fixed values.
#[derive(Debug, Default)]
pub struct MockConnector;

#[async_trait::async_trait]
impl Connector for MockConnector {
    fn id(&self) -> &str {
        "mock"
    }

    fn name(&self) -> &str {
        "Mock Connector"
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Gmail
    }

    fn mode(&self) -> ConnectorMode {
        ConnectorMode::Polling {
            interval: Duration::from_secs(60),
            jitter: Duration::from_secs(5),
        }
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        Ok(ConnectorAuthState::Authenticated)
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        Ok(HealthStatus::Online)
    }

    async fn sync(&self, _options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        Ok(SyncOutcome {
            fetched: 0,
            new_cursor: None,
            fetched_at: Utc::now(),
        })
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        Ok(Vec::new())
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        Ok(())
    }
}

/// [`ConnectorFactory`](crate::ConnectorFactory) that produces
/// [`MockConnector`]s.
///
/// Always-compiled, so the registry can be exercised under every feature
/// combination (including `--no-default-features`). F13 will replace the mock
/// with a configurable harness; this factory simply hands back the no-op
/// [`MockConnector`], ignoring `config`.
#[derive(Debug, Default)]
pub struct MockConnectorFactory;

impl crate::connector::ConnectorFactory for MockConnectorFactory {
    fn create(
        &self,
        _config: serde_json::Value,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError> {
        Ok(std::sync::Arc::new(MockConnector) as std::sync::Arc<dyn Connector>)
    }
}
