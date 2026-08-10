use crate::connector::{Connector, ConnectorError, ConnectorFactory};
use crate::photos::connector::PhotosConnector;

/// [`ConnectorFactory`] that builds a [`PhotosConnector`] from its
/// `config_json`. Gated behind the `photos` feature.
#[derive(Debug, Default)]
pub struct PhotosConnectorFactory;

impl ConnectorFactory for PhotosConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &crate::connector::ConnectorContext,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError> {
        let connector = PhotosConnector::from_config_with_geocoder(config, ctx.geocoder.clone())?;
        Ok(std::sync::Arc::new(connector) as std::sync::Arc<dyn Connector>)
    }
}
