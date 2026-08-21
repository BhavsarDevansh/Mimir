use std::sync::Arc;

use crate::connector::{Connector, ConnectorContext, ConnectorError, ConnectorFactory};
use crate::email::connector::{EmailConnector, EmailConnectorDeps};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Constructs [`EmailConnector`] instances from their persisted `config_json`
/// + the shared [`SecretStore`](crate::secrets::SecretStore) (Phase 3 C5 / #199).
pub struct EmailConnectorFactory;

impl EmailConnectorFactory {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmailConnectorFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectorFactory for EmailConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError> {
        let cursor = config
            .get("__cursor")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let connector = EmailConnector::from_config_with_deps(
            config,
            EmailConnectorDeps {
                secret_store: ctx.secret_store.clone(),
                user_identity: ctx.user_identity.clone(),
                cursor,
                llm_backend: ctx.llm_backend.clone(),
                kg: ctx.knowledge_graph.clone(),
                hook_engine: ctx.hook_engine.clone(),
            },
        )?;
        Ok(Arc::new(connector) as Arc<dyn Connector>)
    }
}
