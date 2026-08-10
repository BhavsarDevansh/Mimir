//! [`LlmBackend`] trait adapter for [`LlmClient`].

use std::sync::Arc;

use async_trait::async_trait;

use crate::llm::backend::LlmBackend;
use crate::llm::backend::LlmStream;
use crate::llm::client::LlmClient;
use crate::llm::types::{LlmError, Message, Usage};

#[async_trait]
impl LlmBackend for LlmClient {
    async fn shutdown(&self) {
        if let Some(pool) = &self.pool {
            Arc::clone(pool).shutdown().await;
        }
        // Dropping the reqwest::Client closes idle connections.
    }

    async fn chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        self.chat_message(messages, tools).await
    }

    async fn system_chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        self.system_chat_message(messages, tools).await
    }

    async fn chat_stream_with_usage(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LlmStream, LlmError> {
        self.chat_stream_with_usage(messages, tools).await
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

    fn in_flight_count(&self) -> usize {
        match &self.pool {
            Some(pool) => pool.in_flight_count(),
            None => 0,
        }
    }

    fn with_model_override(&self, model: String) -> Option<Arc<dyn LlmBackend>> {
        let mut clone = self.clone();
        clone.config.model = model;
        clone.pool = None;
        Some(Arc::new(clone))
    }

    fn with_temperature_override(&self, temperature: f32) -> Option<Arc<dyn LlmBackend>> {
        let mut clone = self.clone();
        clone.config.temperature = temperature;
        clone.pool = None;
        Some(Arc::new(clone))
    }
}
