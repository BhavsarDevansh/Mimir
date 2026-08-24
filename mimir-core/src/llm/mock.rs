use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::llm::backend::{LlmBackend, LlmStream};
use crate::llm::types::{LlmError, Message, StreamItem, Usage};

/// A recorded LLM call capturing both messages and tools.
#[derive(Debug, Clone)]
struct CallRecord {
    messages: Vec<Message>,
    tools: Option<Vec<serde_json::Value>>,
}

/// A queued stream response: an immediate admission failure or a sequence of
/// stream items.
type StreamResponse = Result<Vec<Result<StreamItem, LlmError>>, LlmError>;

/// The error a mock returns when its response queue is empty: retries are
/// exhausted and the cause is reported like a provider `503` overload.
fn queue_empty_error() -> LlmError {
    LlmError::RetryExhausted {
        attempts: 1,
        last_error: Box::new(LlmError::Api {
            status: 503,
            body: "mock response queue empty".to_string(),
        }),
    }
}

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
    chat_responses: Mutex<VecDeque<Result<(Message, Usage), LlmError>>>,
    stream_responses: Mutex<VecDeque<StreamResponse>>,
    context_window: Mutex<Option<u32>>,
    user_queue_depth_val: Mutex<usize>,
    system_queue_depth_val: Mutex<usize>,
    worker_threads_val: u8,
    user_queue_has_capacity_val: Mutex<bool>,
    in_flight_count_val: Mutex<usize>,
    chat_records: Mutex<Vec<CallRecord>>,
    stream_records: Mutex<Vec<CallRecord>>,
    /// Records for [`LlmBackend::system_chat_message`] calls, kept separate
    /// from `chat_records` so tests can assert a connector routed its LLM
    /// extraction through the system queue (#201) rather than the user
    /// queue. System calls consume the same queued responses as user
    /// `chat_message` calls.
    system_chat_records: Mutex<Vec<CallRecord>>,
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
                in_flight_count_val: Mutex::new(0),
                chat_records: Mutex::new(Vec::new()),
                stream_records: Mutex::new(Vec::new()),
                system_chat_records: Mutex::new(Vec::new()),
            },
        }
    }

    /// Return all [`Message`] vectors passed to [`LlmBackend::chat`].
    pub fn chat_calls(&self) -> Vec<Vec<Message>> {
        self.chat_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.messages.clone())
            .collect()
    }

    /// Return all tool options passed to [`LlmBackend::chat`].
    pub fn chat_tools(&self) -> Vec<Option<Vec<serde_json::Value>>> {
        self.chat_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.tools.clone())
            .collect()
    }

    /// Return all [`Message`] vectors passed to [`LlmBackend::system_chat_message`].
    ///
    /// Empty when nothing routed through the system queue, so a test can
    /// assert a connector used [`LlmBackend::system_chat_message`] rather
    /// than the user-queue [`LlmBackend::chat_message`] (#201).
    pub fn system_chat_calls(&self) -> Vec<Vec<Message>> {
        self.system_chat_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.messages.clone())
            .collect()
    }

    /// Return all tool options passed to [`LlmBackend::system_chat_message`].
    pub fn system_chat_tools(&self) -> Vec<Option<Vec<serde_json::Value>>> {
        self.system_chat_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.tools.clone())
            .collect()
    }

    /// Return all [`Message`] vectors passed to [`LlmBackend::chat_stream_with_usage`].
    pub fn stream_calls(&self) -> Vec<Vec<Message>> {
        self.stream_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.messages.clone())
            .collect()
    }

    /// Return all tool options passed to [`LlmBackend::chat_stream_with_usage`].
    pub fn stream_tools(&self) -> Vec<Option<Vec<serde_json::Value>>> {
        self.stream_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.tools.clone())
            .collect()
    }
}

impl MockLlmClientBuilder {
    /// Queue a successful chat response.
    pub fn push_chat(self, text: impl Into<String>, usage: Usage) -> Self {
        self.client
            .chat_responses
            .lock()
            .unwrap()
            .push_back(Ok((Message::assistant(text), usage)));
        self
    }

    /// Queue a successful chat response with a full assistant message.
    pub fn push_chat_message(self, message: Message, usage: Usage) -> Self {
        self.client
            .chat_responses
            .lock()
            .unwrap()
            .push_back(Ok((message, usage)));
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
            .push_back(Ok(items));
        self
    }

    /// Queue an immediate stream admission failure (e.g. a full user queue).
    pub fn push_stream_error(self, error: LlmError) -> Self {
        self.client
            .stream_responses
            .lock()
            .unwrap()
            .push_back(Err(error));
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

    /// Set the value returned by [`LlmBackend::in_flight_count`].
    pub fn in_flight_count(self, n: usize) -> Self {
        *self.client.in_flight_count_val.lock().unwrap() = n;
        self
    }

    /// Build the [`MockLlmClient`].
    pub fn build(self) -> MockLlmClient {
        self.client
    }
}

#[async_trait]
impl LlmBackend for MockLlmClient {
    async fn chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        self.chat_records
            .lock()
            .unwrap()
            .push(CallRecord { messages, tools });
        match self.chat_responses.lock().unwrap().pop_front() {
            Some(result) => result,
            None => Err(queue_empty_error()),
        }
    }

    async fn system_chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        self.system_chat_records
            .lock()
            .unwrap()
            .push(CallRecord { messages, tools });
        // System calls reuse the user-call response queue so a test only
        // needs to queue responses once regardless of which queue the
        // caller targets.
        match self.chat_responses.lock().unwrap().pop_front() {
            Some(result) => result,
            None => Err(queue_empty_error()),
        }
    }

    async fn chat_stream_with_usage(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LlmStream, LlmError> {
        self.stream_records
            .lock()
            .unwrap()
            .push(CallRecord { messages, tools });
        match self.stream_responses.lock().unwrap().pop_front() {
            Some(Ok(items)) => {
                let stream = futures::stream::iter(items);
                Ok(Box::pin(stream))
            }
            Some(Err(e)) => Err(e),
            None => Err(queue_empty_error()),
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

    fn in_flight_count(&self) -> usize {
        *self.in_flight_count_val.lock().unwrap()
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

        let (text1, _) = mock.chat(vec![Message::user("a")], None).await.unwrap();
        let (text2, _) = mock.chat(vec![Message::user("b")], None).await.unwrap();

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

        let result = mock.chat(vec![Message::user("x")], None).await;
        assert!(matches!(result, Err(LlmError::QueueFull)));
    }

    #[tokio::test]
    async fn test_chat_empty_queue_returns_retry_exhausted() {
        let mock = MockLlmClient::builder().build();

        let result = mock.chat(vec![Message::user("x")], None).await;
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
            .chat_stream_with_usage(vec![Message::user("x")], None)
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

        let result = mock
            .chat_stream_with_usage(vec![Message::user("x")], None)
            .await;
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

    #[tokio::test]
    async fn test_system_chat_records_separately_from_user_chat() {
        // A connector LLM call routes through `system_chat_message`, which the
        // mock records apart from user-queue `chat_message` so a test can
        // assert the routing (#201). System calls reuse the queued responses.
        let mock = MockLlmClient::builder()
            .push_chat("system-reply", Usage::default())
            .push_chat("user-reply", Usage::default())
            .build();

        let (sys_text, _) = mock
            .system_chat(vec![Message::system("extract facts")], None)
            .await
            .unwrap();
        assert_eq!(sys_text, "system-reply");

        let (usr_text, _) = mock.chat(vec![Message::user("hi")], None).await.unwrap();
        assert_eq!(usr_text, "user-reply");

        // The system call landed in the system record, not the user record.
        assert_eq!(mock.system_chat_calls().len(), 1);
        assert_eq!(
            mock.system_chat_calls()[0],
            vec![Message::system("extract facts")]
        );
        assert_eq!(mock.chat_calls().len(), 1);
        assert_eq!(mock.chat_calls()[0], vec![Message::user("hi")]);
    }
}
