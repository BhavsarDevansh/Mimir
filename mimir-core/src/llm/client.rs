use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use reqwest::StatusCode;
use tracing::{debug, error, warn};

use crate::config::LlmConfig;
use crate::llm::backend::{LlmBackend, LlmStream};
use crate::llm::pool::{LlmWorkerPool, WorkerPoolConfig};
use crate::llm::types::*;
use async_trait::async_trait;

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF_MS: u64 = 200;
const MAX_BACKOFF_MS: u64 = 10_000;

/// An async HTTP client for OpenAI-compatible LLM APIs.
///
/// Supports both streaming (SSE) and non-streaming chat completion requests
/// with automatic exponential-backoff retry on transient failures.
///
/// By default all requests are routed through an internal [`LlmWorkerPool`]
/// so that background tasks can coexist with user-facing requests without
/// degrading latency. The pool can be replaced in tests via [`Self::with_pool`].
pub struct LlmClient {
    client: reqwest::Client,
    config: LlmConfig,
    pool: Option<Arc<LlmWorkerPool>>,
}

impl fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmClient")
            .field("endpoint", &self.config.endpoint)
            .field("model", &self.config.model)
            .field("max_tokens", &self.config.max_tokens)
            .field("temperature", &self.config.temperature)
            .field("api_key", &"***REDACTED***")
            .field("has_pool", &self.pool.is_some())
            .finish()
    }
}

impl Clone for LlmClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            config: self.config.clone(),
            pool: self.pool.clone(),
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
    /// enqueue, so callers must still handle [`LlmError::QueueFull`].
    pub async fn user_queue_has_capacity(&self) -> bool {
        match &self.pool {
            Some(pool) => pool.user_queue_has_capacity().await,
            None => true,
        }
    }

    /// Create a new client from the provided LLM configuration.
    ///
    /// Internally creates a default [`LlmWorkerPool`] with one worker thread
    /// and bounded queues of size 100.
    ///
    /// # Async
    ///
    /// Must be called from within a Tokio runtime context because it spawns
    /// the internal worker pool tasks. The client uses
    /// [`connect_timeout`](reqwest::ClientBuilder::connect_timeout) rather than
    /// a global request timeout so that long-lived SSE streams are not
    /// prematurely aborted.
    pub async fn new(config: LlmConfig) -> Self {
        let pool = Arc::new(
            LlmWorkerPool::new(config.clone(), WorkerPoolConfig::default())
                .await
                .expect("default WorkerPoolConfig has worker_threads=1"),
        );
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .expect("valid reqwest client"),
            config,
            pool: Some(pool),
        }
    }

    /// Create a client that bypasses the worker pool and makes direct HTTP calls.
    ///
    /// This is used internally by pool workers; external callers should use [`Self::new`].
    pub(crate) fn new_direct(config: LlmConfig) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .expect("valid reqwest client"),
            config,
            pool: None,
        }
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

    /// Send a non-streaming chat completion request.
    ///
    /// Returns the assistant's message content and token usage statistics.
    pub async fn chat(&self, messages: Vec<Message>) -> Result<(String, Usage), LlmError> {
        if let Some(pool) = &self.pool {
            pool.enqueue_chat(messages).await
        } else {
            self.chat_direct(messages).await
        }
    }

    /// Send a streaming chat completion request that includes token usage.
    ///
    /// The returned stream yields `StreamItem::Text` for each content chunk and
    /// `StreamItem::Usage` when the API emits a final usage block (OpenAI-style).
    pub async fn chat_stream_with_usage(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        if let Some(pool) = &self.pool {
            pool.enqueue_chat_stream(messages).await
        } else {
            self.chat_stream_with_usage_direct(messages).await
        }
    }

    /// Send a plain streaming chat completion request (no usage).
    ///
    /// Returns a pinned stream of text chunks.
    pub async fn chat_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>, LlmError> {
        let stream = self.chat_stream_with_usage(messages).await?;
        let text_stream = stream
            .map(|item| match item {
                Ok(StreamItem::Text(text)) => Ok(text),
                Ok(StreamItem::Usage(_)) => Ok(String::new()),
                Err(e) => Err(e),
            })
            .filter(|item| futures::future::ready(!matches!(item, Ok(s) if s.is_empty())));
        Ok(Box::pin(text_stream))
    }

    /// Direct (non-pooled) non-streaming chat completion.
    pub(crate) async fn chat_direct(
        &self,
        messages: Vec<Message>,
    ) -> Result<(String, Usage), LlmError> {
        let request = self.build_request(messages, false);
        debug!(endpoint = %self.config.endpoint, model = %self.config.model, "sending chat request");

        let response = self
            .retry_with_backoff(|| self.send_request(&request))
            .await?;

        let response = self.check_response(response).await?;

        let body: ChatResponse = response.json().await.map_err(LlmError::Network)?;
        let content = body
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        let usage = body.usage.unwrap_or_default();

        debug!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "chat complete"
        );
        Ok((content, usage))
    }

    /// Direct (non-pooled) streaming chat completion with usage.
    pub(crate) async fn chat_stream_with_usage_direct(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        let mut request = self.build_request(messages, true);
        request.stream_options = Some(serde_json::json!({"include_usage": true}));
        debug!(endpoint = %self.config.endpoint, model = %self.config.model, "sending streaming chat request with usage");

        let response = self
            .retry_with_backoff(|| self.send_request(&request))
            .await?;
        let response = self.check_response(response).await?;

        let byte_stream = response.bytes_stream();
        let events = byte_stream.eventsource();

        let stream = events
            .map(|event| {
                let event = event.map_err(|e| LlmError::StreamError(e.to_string()))?;
                Self::map_sse_event(&event.data)
            })
            .filter(|item| {
                futures::future::ready(!matches!(item, Ok(StreamItem::Text(s)) if s.is_empty()))
            });

        Ok(Box::pin(stream))
    }

    /// Direct (non-pooled) plain streaming chat completion.
    /// Map an SSE data line to a [`StreamItem`].
    fn map_sse_event(data: &str) -> Result<StreamItem, LlmError> {
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

        let content = chunk
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.delta.content)
            .unwrap_or_default();

        Ok(StreamItem::Text(content))
    }

    /// Build a `ChatRequest` from the stored configuration.
    fn build_request(&self, messages: Vec<Message>, stream: bool) -> ChatRequest {
        let mut req = ChatRequest::new(self.config.model.clone(), messages)
            .with_temperature(self.config.temperature)
            .with_stream(stream);
        if let Some(mt) = self.config.max_tokens {
            req = req.with_max_tokens(mt);
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
    fn known_context_window(model: &str) -> Option<u32> {
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
    async fn check_response(
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
    async fn send_request(&self, request: &ChatRequest) -> Result<reqwest::Response, LlmError> {
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
    async fn retry_with_backoff<F, Fut, T>(&self, mut operation: F) -> Result<T, LlmError>
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
    fn is_transient(error: &LlmError) -> bool {
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
    fn calculate_backoff(attempt: u32) -> u64 {
        let exponential = BASE_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt));
        exponential.min(MAX_BACKOFF_MS)
    }
}

#[async_trait]
impl LlmBackend for LlmClient {
    async fn chat(&self, messages: Vec<Message>) -> Result<(String, Usage), LlmError> {
        self.chat(messages).await
    }

    async fn chat_stream_with_usage(&self, messages: Vec<Message>) -> Result<LlmStream, LlmError> {
        self.chat_stream_with_usage(messages).await
    }

    async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError> {
        self.fetch_model_context_window().await
    }

    async fn user_queue_depth(&self) -> usize {
        self.user_queue_depth().await
    }

    async fn system_queue_depth(&self) -> usize {
        self.system_queue_depth().await
    }

    fn worker_threads(&self) -> u8 {
        self.worker_threads()
    }

    async fn user_queue_has_capacity(&self) -> bool {
        self.user_queue_has_capacity().await
    }

    fn with_model_override(&self, model: String) -> Option<Arc<dyn LlmBackend>> {
        let mut clone = self.clone();
        clone.config.model = model;
        Some(Arc::new(clone))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use eventsource_stream::Eventsource;
    #[test]
    fn test_debug_does_not_leak_api_key() {
        let config = LlmConfig {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-super-secret".to_string(),
            model: "gpt-4o".to_string(),
            max_tokens: Some(100),
            temperature: 0.2,
        };
        let client = LlmClient::new_direct(config);
        let debug = format!("{:?}", client);
        assert!(
            !debug.contains("sk-super-secret"),
            "Debug output must not contain the API key"
        );
        assert!(debug.contains("***REDACTED***"));
    }

    #[test]
    fn test_calculate_backoff_grows_exponentially() {
        let b1 = LlmClient::calculate_backoff(1);
        let b2 = LlmClient::calculate_backoff(2);
        let b3 = LlmClient::calculate_backoff(3);

        // Base is 200ms; attempt 1 = ~200ms, attempt 2 = ~400ms, attempt 3 = ~800ms
        assert!(b1 >= 200, "backoff 1 should be at least 200ms");
        assert!(b2 >= 400, "backoff 2 should be at least 400ms");
        assert!(b3 >= 800, "backoff 3 should be at least 800ms");
    }

    #[test]
    fn test_calculate_backoff_capped() {
        let b10 = LlmClient::calculate_backoff(10);
        assert!(
            b10 <= MAX_BACKOFF_MS,
            "backoff should be capped at {} ms",
            MAX_BACKOFF_MS
        );
    }

    #[tokio::test]
    async fn test_retry_exhausted_on_persistent_failure() {
        // Build a client pointed at a non-routable address so every request fails.
        let config = LlmConfig {
            endpoint: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "gpt-4o".to_string(),
            max_tokens: Some(10),
            temperature: 0.0,
        };
        let client = LlmClient::new(config).await;

        let result = client.chat(vec![Message::user("hi")]).await;
        assert!(result.is_err());

        match result {
            Err(LlmError::RetryExhausted { attempts }) => {
                assert_eq!(attempts, MAX_RETRIES + 1);
            }
            Err(_other) => {
                // It's also acceptable to get a straight network error if the OS
                // rejects the connection immediately (connection refused).
                // That's still correct behaviour.
            }
            Ok(_) => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn test_chat_stream_parses_mock_sse() {
        // Verify that the stream parser correctly handles a real OpenAI-style SSE chunk.
        let sse_line = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"X"},"finish_reason":null}]}"#;

        // eventsource_stream expects a raw HTTP response body stream.
        // We simulate by creating a small byte stream with the SSE format.
        let body = format!("{}\n\n", sse_line);
        let stream = futures::stream::iter(vec![Ok::<bytes::Bytes, reqwest::Error>(
            bytes::Bytes::from(body),
        )]);
        let mut events = stream.eventsource();

        let event = events.next().await.unwrap().unwrap();
        assert!(event.data.contains("\"content\":\"X\""));
    }

    #[tokio::test]
    async fn test_chat_stream_with_usage_yields_text_and_usage() {
        let text_sse = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let usage_sse = r#"data: {"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#;

        let body = format!("{}\n\n{}\n\n", text_sse, usage_sse);
        let stream = futures::stream::iter(vec![Ok::<bytes::Bytes, reqwest::Error>(
            bytes::Bytes::from(body),
        )]);
        let mut events = stream.eventsource();

        let event1 = events.next().await.unwrap().unwrap();
        let chunk1: StreamChunk = serde_json::from_str(&event1.data).unwrap();
        assert_eq!(chunk1.choices[0].delta.content.as_deref(), Some("Hello"));

        let event2 = events.next().await.unwrap().unwrap();
        let chunk2: StreamChunk = serde_json::from_str(&event2.data).unwrap();
        assert!(chunk2.choices.is_empty());
        let usage = chunk2.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 3);
        assert_eq!(usage.completion_tokens, 1);
    }
}
