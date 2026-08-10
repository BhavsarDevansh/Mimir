//! HTTP transport: request building, SSE parsing, retry/backoff, and
//! context-window introspection.

use std::time::Duration;

use reqwest::StatusCode;
use tracing::{error, warn};

use crate::llm::client::LlmClient;
use crate::llm::types::*;

pub(super) const MAX_RETRIES: u32 = 3;
pub(super) const BASE_BACKOFF_MS: u64 = 200;
pub(super) const MAX_BACKOFF_MS: u64 = 10_000;

impl LlmClient {
    pub(super) fn map_sse_event(data: &str) -> Result<StreamItem, LlmError> {
        if data == "[DONE]" {
            return Ok(StreamItem::Text(String::new()));
        }

        let chunk: StreamChunk = serde_json::from_str(data).map_err(LlmError::Parse)?;

        // Some providers emit usage as a final chunk with empty choices.
        if let Some(usage) = chunk.usage
            && chunk.choices.is_empty()
        {
            return Ok(StreamItem::Usage(usage));
        }

        let choice = chunk.choices.into_iter().next();

        // Check for tool-call deltas first.
        if let Some(ref c) = choice
            && let Some(ref tool_calls) = c.delta.tool_calls
            && !tool_calls.is_empty()
        {
            return Ok(StreamItem::ToolCalls(tool_calls.clone()));
        }

        let content = choice.and_then(|c| c.delta.content).unwrap_or_default();

        Ok(StreamItem::Text(content))
    }

    /// Build a `ChatRequest` from the stored configuration.
    pub(super) fn build_request(
        &self,
        messages: Vec<Message>,
        stream: bool,
        tools: Option<Vec<serde_json::Value>>,
    ) -> ChatRequest {
        let mut req = ChatRequest::new(self.config.model.clone(), messages)
            .with_temperature(self.config.temperature)
            .with_stream(stream);
        if let Some(mt) = self.config.max_tokens {
            req = req.with_max_tokens(mt);
        }
        if let Some(tools) = tools {
            req = req.with_tools(tools);
        }
        req
    }

    /// Query the provider's `/models` endpoint for the configured model
    /// and return its advertised context window (if any).
    ///
    /// Falls back to a small built-in mapping for well-known models when the
    /// provider does not expose `context_window`.
    pub async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError> {
        let url = format!("{}/models/{}", self.config.endpoint, self.config.model);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .send()
            .await
            .map_err(LlmError::Network)?;

        if !response.status().is_success() {
            // Provider doesn't support model introspection — fall back to known mapping.
            return Ok(Self::known_context_window(&self.config.model));
        }

        let info: crate::llm::types::ModelInfo =
            response.json().await.map_err(LlmError::Network)?;
        Ok(info
            .context_window
            .or_else(|| Self::known_context_window(&self.config.model)))
    }

    /// Built-in context-window sizes for popular models.
    pub(super) fn known_context_window(model: &str) -> Option<u32> {
        match model {
            "gpt-4o" | "gpt-4o-mini" => Some(128_000),
            "gpt-4-turbo" => Some(128_000),
            "gpt-4" | "gpt-4-32k" => Some(32_768),
            "gpt-3.5-turbo" | "gpt-3.5-turbo-16k" => Some(16_384),
            "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" => Some(200_000),
            "claude-3-opus" | "claude-3-opus-20240229" => Some(200_000),
            "claude-3-sonnet" | "claude-3-sonnet-20240229" => Some(200_000),
            "claude-3-haiku" | "claude-3-haiku-20240307" => Some(200_000),
            _ => None,
        }
    }

    /// Check that the HTTP response status is successful; otherwise return an error.
    ///
    /// Takes ownership of the response because reading the error body consumes it.
    pub(super) async fn check_response(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, LlmError> {
        if response.status().is_success() {
            Ok(response)
        } else {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            error!(status = status, body = %body, "LLM API returned error");
            Err(LlmError::Api { status, body })
        }
    }

    /// Send the HTTP request for a chat completion.
    ///
    /// Returns `Err(LlmError::Api)` for any non-success HTTP status so that
    /// transient codes (429 / 502 / 503 / 504) are visible to the retry logic.
    pub(super) async fn send_request(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, LlmError> {
        let response = self
            .client
            .post(format!("{}/chat/completions", self.config.endpoint))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(LlmError::Network)?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }

        Ok(response)
    }

    /// Retry an async operation with exponential backoff.
    pub(super) async fn retry_with_backoff<F, Fut, T>(
        &self,
        mut operation: F,
    ) -> Result<T, LlmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, LlmError>>,
    {
        let mut attempt = 0u32;

        loop {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    attempt += 1;

                    if attempt > MAX_RETRIES {
                        error!(attempts = attempt, "retry exhausted");
                        return Err(LlmError::RetryExhausted { attempts: attempt });
                    }

                    if !Self::is_transient(&e) {
                        return Err(e);
                    }

                    let backoff = Self::calculate_backoff(attempt);
                    warn!(
                        attempt = attempt,
                        backoff_ms = backoff,
                        error = %e,
                        "transient error, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
            }
        }
    }

    /// Determine whether an error is transient and should be retried.
    pub(super) fn is_transient(error: &LlmError) -> bool {
        match error {
            LlmError::Network(e) => {
                if e.is_timeout() || e.is_connect() || e.is_request() {
                    return true;
                }
                if let Some(status) = e.status() {
                    return matches!(
                        status,
                        StatusCode::TOO_MANY_REQUESTS
                            | StatusCode::BAD_GATEWAY
                            | StatusCode::SERVICE_UNAVAILABLE
                            | StatusCode::GATEWAY_TIMEOUT
                    );
                }
                false
            }
            LlmError::Api { status, .. } => matches!(*status, 429 | 502 | 503 | 504),
            _ => false,
        }
    }

    /// Calculate the backoff duration for a given attempt number.
    pub(super) fn calculate_backoff(attempt: u32) -> u64 {
        let exponential = BASE_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt));
        exponential.min(MAX_BACKOFF_MS)
    }
}
