//! OpenAI-compatible HTTP chat client.
//!
//! [`LlmClient`] speaks the OpenAI chat-completions API (non-streaming and
//! SSE streaming) with exponential-backoff retries on transient failures.
//! Construction lives in `construct`, request/response marshalling in
//! `transport`, the public chat surface in `chat`, and the
//! [`LlmBackend`](crate::llm::backend::LlmBackend) trait adapter in
//! `backend`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::config::LlmConfig;
use crate::llm::pool::LlmWorkerPool;
use crate::llm::types::LlmRequestOverrides;
use transport::{BASE_BACKOFF_MS, MAX_BACKOFF_MS, MAX_RETRIES};

mod backend;
mod chat;
mod construct;
#[cfg(test)]
mod tests;
mod transport;

/// An async HTTP client for OpenAI-compatible LLM APIs.
///
/// Supports both streaming (SSE) and non-streaming chat completion requests
/// with automatic exponential-backoff retry on transient failures.
///
/// By default all requests are routed through an internal [`LlmWorkerPool`]
/// so that background tasks can coexist with user-facing requests without
/// degrading latency. The pool can be replaced in tests via `Self::with_pool`.
/// Per-request overrides (`with_model_override`, `with_temperature_override`,
/// `with_max_tokens_override`) keep the pool and travel with the job payload,
/// so overridden requests still enqueue on the user queue (issue #465).
pub struct LlmClient {
    client: reqwest::Client,
    config: LlmConfig,
    retry_config: RetryConfig,
    pool: Option<Arc<LlmWorkerPool>>,
    overrides: LlmRequestOverrides,
}

/// Retry schedule for transient LLM failures.
///
/// `max_attempts` includes the initial attempt. The default preserves the
/// historical three-retry schedule and 10-second backoff ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    /// Total attempts, including the initial call. Must be greater than zero.
    pub max_attempts: u8,
    /// Delay before the first retry, before exponential growth.
    pub base_backoff: Duration,
    /// Upper bound applied to every calculated retry delay.
    pub max_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: u8::try_from(MAX_RETRIES + 1).expect("default retry count fits u8"),
            base_backoff: Duration::from_millis(BASE_BACKOFF_MS),
            max_backoff: Duration::from_millis(MAX_BACKOFF_MS),
        }
    }
}

impl RetryConfig {
    fn validated(self) -> Result<Self, String> {
        if self.max_attempts == 0 {
            return Err("RetryConfig.max_attempts must be > 0".to_string());
        }
        Ok(self)
    }
}

impl fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmClient")
            .field("endpoint", &self.config.endpoint)
            .field("model", &self.config.model)
            .field("max_tokens", &self.config.max_tokens)
            .field("temperature", &self.config.temperature)
            .field("retry_config", &self.retry_config)
            .field("api_key", &"***REDACTED***")
            .field("has_pool", &self.pool.is_some())
            .field("overrides", &self.overrides)
            .finish()
    }
}

impl Clone for LlmClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            config: self.config.clone(),
            retry_config: self.retry_config,
            pool: self.pool.clone(),
            overrides: self.overrides.clone(),
        }
    }
}

impl LlmClient {
    // ------------------------------------------------------------------
    // Runtime introspection helpers
    // ------------------------------------------------------------------

    /// Current depth of the user-facing queue.
    pub async fn user_queue_depth(&self) -> usize {
        match &self.pool {
            Some(pool) => pool.user_queue_depth().await,
            None => 0,
        }
    }

    /// Current depth of the system queue.
    pub async fn system_queue_depth(&self) -> usize {
        match &self.pool {
            Some(pool) => pool.system_queue_depth().await,
            None => 0,
        }
    }

    /// Number of worker threads in the backing pool.
    pub fn worker_threads(&self) -> u8 {
        match &self.pool {
            Some(pool) => pool.worker_threads(),
            None => 0,
        }
    }

    /// Best-effort check whether the user queue has capacity.
    ///
    /// A concurrent request can fill the gap between this check and the actual
    /// enqueue, so callers must still handle [`LlmError::QueueFull`](crate::llm::types::LlmError::QueueFull).
    pub async fn user_queue_has_capacity(&self) -> bool {
        match &self.pool {
            Some(pool) => pool.user_queue_has_capacity().await,
            None => true,
        }
    }
}
