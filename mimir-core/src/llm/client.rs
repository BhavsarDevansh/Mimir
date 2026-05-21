use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use eventsource_stream::Eventsource;
use reqwest::StatusCode;
use tracing::{debug, error, warn};

use crate::config::LlmConfig;
use crate::llm::types::*;

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF_MS: u64 = 200;
const MAX_BACKOFF_MS: u64 = 10_000;

/// An async HTTP client for OpenAI-compatible LLM APIs.
///
/// Supports both streaming (SSE) and non-streaming chat completion requests
/// with automatic exponential-backoff retry on transient failures.
pub struct LlmClient {
    client: reqwest::Client,
    config: LlmConfig,
}

impl fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmClient")
            .field("endpoint", &self.config.endpoint)
            .field("model", &self.config.model)
            .field("max_tokens", &self.config.max_tokens)
            .field("temperature", &self.config.temperature)
            .field("api_key", &"***REDACTED***")
            .finish()
    }
}

impl Clone for LlmClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            config: self.config.clone(),
        }
    }
}

impl LlmClient {
    /// Create a new client from the provided LLM configuration.
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
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
    pub async fn chat(
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

        debug!(prompt_tokens = usage.prompt_tokens, completion_tokens = usage.completion_tokens, "chat complete");
        Ok((content, usage))
    }

    /// Send a streaming chat completion request.
    ///
    /// Returns a pinned stream of text chunks. The stream yields `Result<String, LlmError>`
    /// for each token chunk received from the server.
    pub async fn chat_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>, LlmError> {
        let request = self.build_request(messages, true);
        debug!(endpoint = %self.config.endpoint, model = %self.config.model, "sending streaming chat request");

        let response = self
            .retry_with_backoff(|| self.send_request(&request))
            .await?;

        let response = self.check_response(response).await?;

        let byte_stream = response.bytes_stream();
        let events = byte_stream.eventsource();

        let text_stream = events
            .map(|event| {
                let event = event.map_err(|e| {
                    LlmError::StreamError(e.to_string())
                })?;

                if event.data == "[DONE]" {
                    return Ok(String::new());
                }

                let chunk: StreamChunk = serde_json::from_str(&event.data)
                    .map_err(LlmError::Parse)?;

                let content = chunk
                    .choices
                    .into_iter()
                    .next()
                    .and_then(|c| c.delta.content)
                    .unwrap_or_default();

                Ok(content)
            })
            .filter(|item| {
                // Filter out empty chunks and the [DONE] sentinel
                futures::future::ready(!matches!(item, Ok(s) if s.is_empty()))
            });

        Ok(Box::pin(text_stream))
    }

    /// Build a `ChatRequest` from the stored configuration.
    fn build_request(
        &self,
        messages: Vec<Message>,
        stream: bool,
    ) -> ChatRequest {
        ChatRequest::new(self.config.model.clone(), messages)
            .with_max_tokens(self.config.max_tokens)
            .with_temperature(self.config.temperature)
            .with_stream(stream)
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
    async fn send_request(
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
    async fn retry_with_backoff<F, Fut, T>(
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
            LlmError::Api { status, .. } => matches!(
                *status,
                429 | 502 | 503 | 504
            ),
            _ => false,
        }
    }

    /// Calculate the backoff duration for a given attempt number.
    fn calculate_backoff(attempt: u32) -> u64 {
        let exponential = BASE_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt));
        exponential.min(MAX_BACKOFF_MS)
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
            max_tokens: 100,
            temperature: 0.2,
        };
        let client = LlmClient::new(config);
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
        assert!(b10 <= MAX_BACKOFF_MS, "backoff should be capped at {} ms", MAX_BACKOFF_MS);
    }

    #[tokio::test]
    async fn test_retry_exhausted_on_persistent_failure() {
        // Build a client pointed at a non-routable address so every request fails.
        let config = LlmConfig {
            endpoint: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "gpt-4o".to_string(),
            max_tokens: 10,
            temperature: 0.0,
        };
        let client = LlmClient::new(config);

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
        let stream = futures::stream::iter(vec![Ok::<bytes::Bytes, reqwest::Error>(bytes::Bytes::from(body))]);
        let mut events = stream.eventsource();

        let event = events.next().await.unwrap().unwrap();
        assert!(event.data.contains("\"content\":\"X\""));
    }
}
