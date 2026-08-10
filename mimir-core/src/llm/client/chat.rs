//! Chat-completion surface: pooled + direct non-streaming and SSE streaming.

use std::pin::Pin;

use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use tracing::debug;

use crate::llm::client::LlmClient;
use crate::llm::types::{ChatResponse, LlmError, Message, StreamItem, Usage};

impl LlmClient {
    pub async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(String, Usage), LlmError> {
        if let Some(pool) = &self.pool {
            pool.enqueue_chat(messages, tools).await
        } else {
            self.chat_direct(messages, tools).await
        }
    }

    /// Send a non-streaming chat completion request and return the full assistant message.
    pub async fn chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        if let Some(pool) = &self.pool {
            pool.enqueue_chat_message(messages, tools).await
        } else {
            self.chat_message_direct(messages, tools).await
        }
    }

    /// Send a non-streaming chat completion request on the **system queue**.
    ///
    /// Routes through the backing [`LlmWorkerPool`](crate::llm::pool::LlmWorkerPool)'s system queue when pooled,
    /// so the call runs at lower priority than user chat (a queued user job is
    /// drained first). When the client has no pool (model-override clones,
    /// direct test clients) the call runs synchronously via
    /// `Self::chat_message_direct` — the system/user
    /// distinction only exists for a pooled client.
    pub async fn system_chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        if let Some(pool) = &self.pool {
            pool.enqueue_system_chat_message(messages, tools).await
        } else {
            self.chat_message_direct(messages, tools).await
        }
    }

    /// Send a streaming chat completion request that includes token usage.
    ///
    /// The returned stream yields `StreamItem::Text` for each content chunk and
    /// `StreamItem::Usage` when the API emits a final usage block (OpenAI-style).
    pub async fn chat_stream_with_usage(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        if let Some(pool) = &self.pool {
            pool.enqueue_chat_stream(messages, tools).await
        } else {
            self.chat_stream_with_usage_direct(messages, tools).await
        }
    }

    /// Send a plain streaming chat completion request (no usage).
    ///
    /// Returns a pinned stream of text chunks.
    pub async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>, LlmError> {
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

    /// Direct (non-pooled) non-streaming chat completion.
    pub(crate) async fn chat_direct(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self.chat_message_direct(messages, tools).await?;
        Ok((msg.content, usage))
    }

    /// Direct (non-pooled) non-streaming chat completion returning the full message.
    pub(crate) async fn chat_message_direct(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        let request = self.build_request(messages, false, tools);
        debug!(endpoint = %self.config.endpoint, model = %self.config.model, "sending chat request");

        let response = self
            .retry_with_backoff(|| self.send_request(&request))
            .await?;

        let response = self.check_response(response).await?;

        let body: ChatResponse = response.json().await.map_err(LlmError::Network)?;
        let message = body
            .choices
            .into_iter()
            .next()
            .map(|c| c.message)
            .unwrap_or_else(|| Message::assistant(""));
        let usage = body.usage.unwrap_or_default();

        debug!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "chat complete"
        );
        Ok((message, usage))
    }

    /// Direct (non-pooled) streaming chat completion with usage.
    pub(crate) async fn chat_stream_with_usage_direct(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        let mut request = self.build_request(messages, true, tools);
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
}
