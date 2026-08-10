use mimir_api_types::{AddConnectorRequest, ConnectorListResponse, ConnectorResponse};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    // -----------------------------------------------------------------
    // Connector management (Phase 3 A1 / #202)
    // -----------------------------------------------------------------
    /// List every registered connector instance with derived item counts.
    pub async fn connectors(&self) -> Result<ConnectorListResponse, ClientError> {
        self.get_json(&self.url("connectors"), &()).await
    }

    /// Fetch a single connector instance by id.
    pub async fn connector(&self, id: i32) -> Result<ConnectorResponse, ClientError> {
        self.get_json(&self.url(&format!("connectors/{id}")), &())
            .await
    }

    /// Register a new connector instance. The daemon validates the
    /// `(connector_type, backend)` pair and rejects an existing slug.
    pub async fn connector_add(
        &self,
        req: AddConnectorRequest,
    ) -> Result<ConnectorResponse, ClientError> {
        self.post_json(&self.url("connectors"), &req).await
    }

    /// Delete a connector instance, detaching its provenance.
    pub async fn connector_remove(&self, id: i32) -> Result<(), ClientError> {
        Self::check_status(
            self.client
                .delete(self.url(&format!("connectors/{id}")))
                .send()
                .await?,
        )
        .await
    }

    /// Trigger a manual sync of a connector instance (Phase 3 A2 / #203).
    pub async fn connector_sync(
        &self,
        id: i32,
        req: mimir_api_types::SyncConnectorRequest,
    ) -> Result<mimir_api_types::SyncConnectorResponse, ClientError> {
        self.post_json(&self.url(&format!("connectors/{id}/sync")), &req)
            .await
    }

    /// Pause a connector instance (Phase 3 A2 / #203).
    pub async fn connector_pause(
        &self,
        id: i32,
    ) -> Result<mimir_api_types::ConnectorResponse, ClientError> {
        self.post_json(
            &self.url(&format!("connectors/{id}/pause")),
            &serde_json::json!({}),
        )
        .await
    }

    /// Resume a connector instance (Phase 3 A2 / #203).
    pub async fn connector_resume(
        &self,
        id: i32,
    ) -> Result<mimir_api_types::ConnectorResponse, ClientError> {
        self.post_json(
            &self.url(&format!("connectors/{id}/resume")),
            &serde_json::json!({}),
        )
        .await
    }

    /// Ingest credentials for a connector instance (Phase 3 A2 / #203).
    pub async fn connector_tokens(
        &self,
        id: i32,
        req: mimir_api_types::IngestTokenRequest,
    ) -> Result<mimir_api_types::ConnectorResponse, ClientError> {
        self.post_json(&self.url(&format!("connectors/{id}/tokens")), &req)
            .await
    }

    /// Dispatch a write-back action to a connector instance (Phase 3 A2 / #203).
    pub async fn connector_actions(
        &self,
        id: i32,
        req: mimir_api_types::ConnectorActionRequest,
    ) -> Result<mimir_api_types::ActionResultResponse, ClientError> {
        self.post_json(&self.url(&format!("connectors/{id}/actions")), &req)
            .await
    }

    /// Cascade-forget a connector instance (Phase 3 A2 / #203).
    pub async fn connector_forget(
        &self,
        id: i32,
    ) -> Result<mimir_api_types::ForgetConnectorResponse, ClientError> {
        self.post_json(
            &self.url(&format!("connectors/{id}/forget")),
            &serde_json::json!({}),
        )
        .await
    }
}
