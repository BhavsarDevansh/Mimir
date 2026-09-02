//! [`LlmClient`] construction: worker-pool and direct HTTP variants.

use std::sync::Arc;
use std::time::Duration;

use crate::config::LlmConfig;
use crate::llm::client::{LlmClient, RetryConfig};
use crate::llm::pool::{LlmWorkerPool, WorkerPoolConfig};
use crate::llm::types::{LlmError, LlmRequestOverrides};

impl LlmClient {
    pub async fn new(config: LlmConfig) -> Result<Self, LlmError> {
        Self::new_with_retry_config(config, RetryConfig::default()).await
    }

    /// Create a client with an explicit retry schedule.
    ///
    /// The schedule applies to direct calls and to every worker in the pool.
    pub async fn new_with_retry_config(
        config: LlmConfig,
        retry_config: RetryConfig,
    ) -> Result<Self, LlmError> {
        Self::new_with_pool_config(config, WorkerPoolConfig::default(), retry_config).await
    }

    /// Create a new client with an explicit [`WorkerPoolConfig`].
    ///
    /// Like [`Self::new`] but lets tests (and future embedders) control the
    /// worker pool shape. A failure to initialise the pool or build the HTTP
    /// client surfaces as [`LlmError::ClientBuild`] instead of panicking
    /// (issue #166).
    pub(super) async fn new_with_pool_config(
        config: LlmConfig,
        pool_config: WorkerPoolConfig,
        retry_config: RetryConfig,
    ) -> Result<Self, LlmError> {
        // Build the HTTP client first so a failure cannot leave already-spawned
        // workers detached (PR #177 review: build HTTP client before spawning
        // workers; `LlmWorkerPool` has no Drop cleanup on this path).
        let client = Self::build_reqwest_client()?;
        let pool = Arc::new(
            LlmWorkerPool::new(config.clone(), pool_config, retry_config)
                .await
                .map_err(|e| LlmError::ClientBuild(format!("worker pool init: {e}")))?,
        );
        Ok(Self {
            client,
            config,
            retry_config,
            pool: Some(pool),
            overrides: LlmRequestOverrides::default(),
        })
    }

    /// Create a client that bypasses the worker pool and makes direct HTTP calls.
    ///
    /// This is used internally by pool workers; external callers should use [`Self::new`].
    pub(crate) fn new_direct(
        config: LlmConfig,
        retry_config: RetryConfig,
    ) -> Result<Self, LlmError> {
        let client = Self::build_reqwest_client()?;
        Ok(Self {
            client,
            config,
            retry_config,
            pool: None,
            overrides: LlmRequestOverrides::default(),
        })
    }

    /// Build the shared `reqwest::Client` used for upstream LLM calls.
    ///
    /// Extracted so a construction failure surfaces as [`LlmError::ClientBuild`]
    /// instead of a startup panic (issue #166). A `connect_timeout` is used
    /// rather than a global request timeout so long-lived SSE streams are not
    /// prematurely aborted.
    fn build_reqwest_client() -> Result<reqwest::Client, LlmError> {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| LlmError::ClientBuild(format!("failed to build reqwest client: {e}")))
    }

    /// Replace the default worker pool with a custom one (test injection).
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn with_pool(mut self, pool: Arc<LlmWorkerPool>) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Create a new client with a custom HTTP client.
    #[cfg(test)]
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }
}
