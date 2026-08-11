use std::sync::Arc;

use crate::connector::{Connector, ConnectorContext, ConnectorError, ConnectorFactory};
use crate::email::connector::EmailConnector;

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
            ctx.secret_store.clone(),
            ctx.user_identity.clone(),
            cursor,
            ctx.llm_backend.clone(),
        )?;
        Ok(Arc::new(connector) as Arc<dyn Connector>)
    }
}
