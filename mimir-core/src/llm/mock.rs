use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::llm::backend::{LlmBackend, LlmStream};
use crate::llm::types::{LlmError, Message, StreamItem, Usage};

/// A programmable mock LLM backend for deterministic, fast tests.
///
/// Responses are queued in FIFO order. Callers can assert on the messages
/// sent to the mock via [`Self::chat_calls`] and [`Self::stream_calls`].
///
/// # Example
/// ```
/// # use mimir_core::llm::MockLlmClient;
/// # use mimir_core::llm::Usage;
/// let mock = MockLlmClient::builder()
///     .push_chat("Hello!", Usage::default())
///     .user_queue_depth(3)
///     .build();
/// assert!(mock.chat_calls().is_empty());
/// ```
#[derive(Debug)]
pub struct MockLlmClient {
    chat_responses: Mutex<VecDeque<Result<(String, Usage), LlmError>>>,
    stream_responses: Mutex<VecDeque<Vec<Result<StreamItem, LlmError>>>>,
    context_window: Mutex<Option<u32>>,
    user_queue_depth_val: Mutex<usize>,
    system_queue_depth_val: Mutex<usize>,
    worker_threads_val: u8,
    user_queue_has_capacity_val: Mutex<bool>,
    chat_calls: Mutex<Vec<Vec<Message>>>,
    stream_calls: Mutex<Vec<Vec<Message>>>,
}

/// Builder for [`MockLlmClient`].
pub struct MockLlmClientBuilder {
    client: MockLlmClient,
}

impl MockLlmClient {
    /// Create a new builder.
    pub fn builder() -> MockLlmClientBuilder {
        MockLlmClientBuilder {
            client: MockLlmClient {
                chat_responses: Mutex::new(VecDeque::new()),
                stream_responses: Mutex::new(VecDeque::new()),
                context_window: Mutex::new(None),
                user_queue_depth_val: Mutex::new(0),
                system_queue_depth_val: Mutex::new(0),
                worker_threads_val: 0,
                user_queue_has_capacity_val: Mutex::new(true),
                chat_calls: Mutex::new(Vec::new()),
                stream_calls: Mutex::new(Vec::new()),
            },
        }
    }

    /// Return all [`Message`] vectors passed to [`LlmBackend::chat`].
    pub fn chat_calls(&self) -> Vec<Vec<Message>> {
        self.chat_calls.lock().unwrap().clone()
    }

    /// Return all [`Message`] vectors passed to [`LlmBackend::chat_stream_with_usage`].
    pub fn stream_calls(&self) -> Vec<Vec<Message>> {
        self.stream_calls.lock().unwrap().clone()
    }
}

impl MockLlmClientBuilder {
    /// Queue a successful chat response.
    pub fn push_chat(self, text: impl Into<String>, usage: Usage) -> Self {
        self.client
            .chat_responses
            .lock()
            .unwrap()
            .push_back(Ok((text.into(), usage)));
        self
    }

    /// Queue a chat failure.
    pub fn push_chat_error(self, error: LlmError) -> Self {
        self.client
            .chat_responses
            .lock()
            .unwrap()
            .push_back(Err(error));
        self
    }

    /// Queue a stream response as a sequence of [`StreamItem`] results.
    pub fn push_stream(self, items: Vec<Result<StreamItem, LlmError>>) -> Self {
        self.client
            .stream_responses
            .lock()
            .unwrap()
            .push_back(items);
        self
    }

    /// Set the value returned by [`LlmBackend::fetch_model_context_window`].
    pub fn context_window(self, window: Option<u32>) -> Self {
        *self.client.context_window.lock().unwrap() = window;
        self
    }

    /// Set the value returned by [`LlmBackend::user_queue_depth`].
    pub fn user_queue_depth(self, n: usize) -> Self {
        *self.client.user_queue_depth_val.lock().unwrap() = n;
        self
    }

    /// Set the value returned by [`LlmBackend::system_queue_depth`].
    pub fn system_queue_depth(self, n: usize) -> Self {
        *self.client.system_queue_depth_val.lock().unwrap() = n;
        self
    }

    /// Set the value returned by [`LlmBackend::worker_threads`].
    pub fn worker_threads(mut self, n: u8) -> Self {
        self.client.worker_threads_val = n;
        self
    }

    /// Set the value returned by [`LlmBackend::user_queue_has_capacity`].
    pub fn user_queue_has_capacity(self, val: bool) -> Self {
        *self.client.user_queue_has_capacity_val.lock().unwrap() = val;
        self
    }

    /// Build the [`MockLlmClient`].
    pub fn build(self) -> MockLlmClient {
        self.client
    }
}

#[async_trait]
impl LlmBackend for MockLlmClient {
    async fn chat(&self, messages: Vec<Message>) -> Result<(String, Usage), LlmError> {
        self.chat_calls.lock().unwrap().push(messages);
        match self.chat_responses.lock().unwrap().pop_front() {
            Some(result) => result,
            None => Err(LlmError::RetryExhausted { attempts: 1 }),
        }
    }

    async fn chat_stream_with_usage(&self, messages: Vec<Message>) -> Result<LlmStream, LlmError> {
        self.stream_calls.lock().unwrap().push(messages);
        match self.stream_responses.lock().unwrap().pop_front() {
            Some(items) => {
                let stream = futures::stream::iter(items);
                Ok(Box::pin(stream))
            }
            None => Err(LlmError::RetryExhausted { attempts: 1 }),
        }
    }

    async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError> {
        Ok(*self.context_window.lock().unwrap())
    }

    async fn user_queue_depth(&self) -> usize {
        *self.user_queue_depth_val.lock().unwrap()
    }

    async fn system_queue_depth(&self) -> usize {
        *self.system_queue_depth_val.lock().unwrap()
    }

    fn worker_threads(&self) -> u8 {
        self.worker_threads_val
    }

    async fn user_queue_has_capacity(&self) -> bool {
        *self.user_queue_has_capacity_val.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{LlmError, Message, StreamItem, Usage};
    use futures::StreamExt;

    #[test]
    fn test_builder_queues_chat_responses() {
        let mock = MockLlmClient::builder()
            .push_chat("A", Usage::default())
            .push_chat("B", Usage::default())
            .build();

        assert_eq!(mock.chat_responses.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_builder_queues_stream_responses() {
        let mock = MockLlmClient::builder()
            .push_stream(vec![Ok(StreamItem::Text("hi".to_string()))])
            .build();

        assert_eq!(mock.stream_responses.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_chat_yields_queued_responses_in_order() {
        let mock = MockLlmClient::builder()
            .push_chat("first", Usage::default())
            .push_chat("second", Usage::default())
            .build();

        let (text1, _) = mock.chat(vec![Message::user("a")]).await.unwrap();
        let (text2, _) = mock.chat(vec![Message::user("b")]).await.unwrap();

        assert_eq!(text1, "first");
        assert_eq!(text2, "second");

        let calls = mock.chat_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], vec![Message::user("a")]);
        assert_eq!(calls[1], vec![Message::user("b")]);
    }

    #[tokio::test]
    async fn test_chat_error_propagates() {
        let mock = MockLlmClient::builder()
            .push_chat_error(LlmError::QueueFull)
            .build();

        let result = mock.chat(vec![Message::user("x")]).await;
        assert!(matches!(result, Err(LlmError::QueueFull)));
    }

    #[tokio::test]
    async fn test_chat_empty_queue_returns_retry_exhausted() {
        let mock = MockLlmClient::builder().build();

        let result = mock.chat(vec![Message::user("x")]).await;
        assert!(matches!(result, Err(LlmError::RetryExhausted { .. })));
    }

    #[tokio::test]
    async fn test_stream_yields_queued_items() {
        let mock = MockLlmClient::builder()
            .push_stream(vec![
                Ok(StreamItem::Text("hello".to_string())),
                Ok(StreamItem::Usage(Usage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                })),
            ])
            .build();

        let mut stream = mock
            .chat_stream_with_usage(vec![Message::user("x")])
            .await
            .unwrap();

        let item1 = stream.next().await.unwrap().unwrap();
        assert!(matches!(item1, StreamItem::Text(t) if t == "hello"));

        let item2 = stream.next().await.unwrap().unwrap();
        assert!(matches!(item2, StreamItem::Usage(u) if u.total_tokens == 2));

        assert!(stream.next().await.is_none());

        let calls = mock.stream_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec![Message::user("x")]);
    }

    #[tokio::test]
    async fn test_stream_empty_queue_returns_retry_exhausted() {
        let mock = MockLlmClient::builder().build();

        let result = mock.chat_stream_with_usage(vec![Message::user("x")]).await;
        assert!(matches!(result, Err(LlmError::RetryExhausted { .. })));
    }

    #[tokio::test]
    async fn test_introspection_setters() {
        let mock = MockLlmClient::builder()
            .user_queue_depth(5)
            .system_queue_depth(3)
            .worker_threads(2)
            .user_queue_has_capacity(false)
            .context_window(Some(4096))
            .build();

        assert_eq!(mock.user_queue_depth().await, 5);
        assert_eq!(mock.system_queue_depth().await, 3);
        assert_eq!(mock.worker_threads(), 2);
        assert!(!mock.user_queue_has_capacity().await);
        assert_eq!(mock.fetch_model_context_window().await.unwrap(), Some(4096));
    }
}
