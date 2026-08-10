//! Optimization job status and run-now commands.

use mimir_api_types::{OptimizationRunNowResponse, OptimizationStatusResponse};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    pub async fn kb_optimization_status(&self) -> Result<OptimizationStatusResponse, ClientError> {
        self.get_json(&self.url("kb/optimization/status"), &())
            .await
    }

    /// Trigger the knowledge graph optimization job immediately.
    pub async fn kb_optimization_run_now(&self) -> Result<OptimizationRunNowResponse, ClientError> {
        Self::send_json(self.client.post(self.url("kb/optimization/run-now"))).await
    }

    // Trigger a graceful shutdown of the daemon.
    // ------------------------------------------------------------------
    // Knowledge Graph (kb) commands
}
