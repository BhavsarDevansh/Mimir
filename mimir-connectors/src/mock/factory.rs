//! [`ConnectorFactory`] implementation for the mock connector.

use super::MockConnector;
use crate::connector::{Connector, ConnectorError, ConnectorFactory};

#[derive(Debug, Default)]
pub struct MockConnectorFactory;

impl ConnectorFactory for MockConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        _ctx: &crate::connector::ConnectorContext,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError> {
        let connector = MockConnector::from_config(config)?;
        Ok(std::sync::Arc::new(connector) as std::sync::Arc<dyn Connector>)
    }
}
