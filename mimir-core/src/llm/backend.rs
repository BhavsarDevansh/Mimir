use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};

use crate::llm::types::{LlmError, Message, StreamItem, Usage};

/// A pinned, boxed stream of LLM responses with usage information.
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>;

/// A pinned, boxed stream of plain text chunks.
pub type LlmTextStream = Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>;

/// Abstract backend for LLM operations.
///
/// Enables fast, deterministic testing via the mock LLM client and insulates
/// server routes from concrete HTTP client details.
#[async_trait]
pub trait LlmBackend: Send + Sync + Debug {
    /// Send a non-streaming chat completion request and return the full assistant message.
    async fn chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError>;

    /// Send a non-streaming chat completion request.
    ///
    /// Default implementation delegates to [`Self::chat_message`] and extracts
    /// the text content.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self.chat_message(messages, tools).await?;
        Ok((msg.content, usage))
    }

    /// Send a non-streaming chat completion request on the **system queue**.
    ///
    /// System-queue calls run at a lower priority than user chat: a pooled
    /// backend drains the user queue before servicing a system job, so a
    /// queued user chat preempts a waiting connector call. Background work —
    /// connector LLM extraction (#201), condensation, inference — routes here
    /// so one-call-at-a-time providers never block interactive chat.
    ///
    /// The default implementation delegates to [`Self::chat_message`] so
    /// backends without a separate system queue (mocks, direct test clients,
    /// model-override clones) run synchronously. Pool-backed backends
    /// (`LlmClient`) override this to enqueue on the system queue.
    async fn system_chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        self.chat_message(messages, tools).await
    }

    /// Send a non-streaming chat completion request on the **system queue**.
    ///
    /// Default implementation delegates to [`Self::system_chat_message`] and
    /// extracts the text content.
    async fn system_chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self.system_chat_message(messages, tools).await?;
        Ok((msg.content, usage))
    }

    /// Send a streaming chat completion request that includes token usage.
    async fn chat_stream_with_usage(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LlmStream, LlmError>;

    /// Send a plain streaming chat completion request (no usage).
    ///
    /// Default implementation delegates to [`Self::chat_stream_with_usage`] and
    /// filters out [`StreamItem::Usage`] events.
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LlmTextStream, LlmError> {
        let stream = self.chat_stream_with_usage(messages, tools).await?;
        let text_stream = stream
            .map(|item| match item {
                Ok(StreamItem::Text(text)) => Ok(text),
                Ok(StreamItem::Usage(_)) | Ok(StreamItem::ToolCalls(_)) => Ok(String::new()),
                Err(e) => Err(e),
            })
            .filter(|item| futures::future::ready(!matches!(item, Ok(s) if s.is_empty())));
        Ok(Box::pin(text_stream))
    }

    /// Query the provider's advertised context window for the configured model.
    async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError>;

    /// Current depth of the user-facing queue.
    async fn user_queue_depth(&self) -> usize {
        0
    }

    /// Current depth of the system queue.
    async fn system_queue_depth(&self) -> usize {
        0
    }

    /// Number of worker threads in the backing pool.
    fn worker_threads(&self) -> u8 {
        0
    }

    /// Best-effort check whether the user queue has capacity.
    async fn user_queue_has_capacity(&self) -> bool {
        true
    }

    /// Number of jobs currently being processed by workers.
    fn in_flight_count(&self) -> usize {
        0
    }

    /// Whether the backing worker pool is completely idle: no queued user or
    /// system jobs and no in-flight jobs.
    ///
    /// Shared by the background scheduler and the hooks engine so idle-gated
    /// background work never steals LLM capacity from interactive chat.
    async fn pool_is_idle(&self) -> bool {
        self.user_queue_depth().await == 0
            && self.system_queue_depth().await == 0
            && self.in_flight_count() == 0
    }

    /// Gracefully shut down the backend, releasing resources.
    ///
    /// The default implementation is a no-op so existing mocks are unaffected.
    async fn shutdown(&self) {}

    /// Return a clone of this backend with the model overridden.
    ///
    /// The default implementation returns `None`, indicating that the backend
    /// does not support model overrides.
    fn with_model_override(&self, _model: String) -> Option<Arc<dyn LlmBackend>> {
        None
    }

    /// Return a clone of this backend with the sampling temperature
    /// overridden.
    ///
    /// This lets callers apply the live configuration temperature per request
    /// so that hot-reloaded temperature changes take effect without restarting
    /// the daemon (issue #80). The default returns `None` (backend ignores the
    /// override and uses its configured temperature).
    fn with_temperature_override(&self, _temperature: f32) -> Option<Arc<dyn LlmBackend>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{LlmError, Message, StreamItem, Usage};

    // A minimal no-op backend for testing the trait default implementations.
    #[derive(Debug)]
    struct DummyBackend;

    #[async_trait]
    impl LlmBackend for DummyBackend {
        async fn chat_message(
            &self,
            _messages: Vec<Message>,
            _tools: Option<Vec<serde_json::Value>>,
        ) -> Result<(Message, Usage), LlmError> {
            Ok((Message::assistant("dummy"), Usage::default()))
        }

        async fn chat_stream_with_usage(
            &self,
            _messages: Vec<Message>,
            _tools: Option<Vec<serde_json::Value>>,
        ) -> Result<LlmStream, LlmError> {
            let items: Vec<Result<StreamItem, LlmError>> = vec![
                Ok(StreamItem::Text("hello".to_string())),
                Ok(StreamItem::Usage(Usage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                })),
            ];
            Ok(Box::pin(futures::stream::iter(items)))
        }

        async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn test_default_chat_stream_filters_usage() {
        let backend = DummyBackend;
        let stream = backend
            .chat_stream(vec![Message::user("hi")], None)
            .await
            .unwrap();
        let items: Vec<Result<String, LlmError>> = stream.collect().await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].as_ref().unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_default_introspection_returns_zero() {
        let backend = DummyBackend;
        assert_eq!(backend.user_queue_depth().await, 0);
        assert_eq!(backend.system_queue_depth().await, 0);
        assert_eq!(backend.worker_threads(), 0);
        assert!(backend.user_queue_has_capacity().await);
    }

    #[tokio::test]
    async fn test_with_model_override_default_returns_none() {
        let backend = DummyBackend;
        assert!(backend.with_model_override("other".to_string()).is_none());
    }
}
