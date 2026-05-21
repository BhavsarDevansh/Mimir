use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A chat completion request compatible with the OpenAI API.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub stream: bool,
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// The response body from a non-streaming chat completion request.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub id: Option<String>,
    pub object: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

/// A single choice in a chat completion response.
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub index: Option<u32>,
    pub message: Message,
    pub finish_reason: Option<String>,
}

/// Token usage statistics for a completion request.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A single chunk from a streaming (SSE) chat completion response.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamChunk {
    pub id: Option<String>,
    pub object: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub choices: Vec<StreamChoice>,
}

/// A single choice within a streaming chunk.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamChoice {
    pub index: Option<u32>,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

/// The delta content in a streaming chunk.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
}

/// Errors that can occur when interacting with an LLM API.
#[derive(Debug, Error)]
pub enum LlmError {
    /// The HTTP request failed (network timeout, DNS error, etc.).
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// The API returned a non-success HTTP status.
    #[error("API error {status}: {body}")]
    Api { status: u16, body: String },

    /// The response body could not be parsed.
    #[error("parse error: {0}")]
    Parse(#[from] serde_json::Error),

    /// All retry attempts were exhausted.
    #[error("retry exhausted after {attempts} attempts")]
    RetryExhausted { attempts: u32 },

    /// The SSE stream produced an invalid event.
    #[error("SSE stream error: {0}")]
    StreamError(String),
}

impl ChatRequest {
    /// Create a new chat request with the given model and messages.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            temperature: None,
            stream: false,
        }
    }

    /// Set the maximum number of tokens to generate.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Enable or disable streaming.
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }
}

impl Message {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_request_serialization() {
        let req = ChatRequest::new("gpt-4o", vec![Message::user("hello")])
            .with_max_tokens(100)
            .with_temperature(0.5)
            .with_stream(false);

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"gpt-4o\""));
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"hello\""));
        assert!(json.contains("\"max_tokens\":100"));
        assert!(json.contains("\"temperature\":0.5"));
        assert!(json.contains("\"stream\":false"));
    }

    #[test]
    fn test_message_constructors() {
        let sys = Message::system("You are helpful");
        assert_eq!(sys.role, "system");
        assert_eq!(sys.content, "You are helpful");

        let usr = Message::user("hi");
        assert_eq!(usr.role, "user");
        assert_eq!(usr.content, "hi");

        let ast = Message::assistant("hello!");
        assert_eq!(ast.role, "assistant");
        assert_eq!(ast.content, "hello!");
    }

    #[test]
    fn test_stream_chunk_parsing() {
        let json = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1677652288,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {"content": "Hello"},
                "finish_reason": null
            }]
        }"#;

        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.id.as_deref(), Some("chatcmpl-123"));
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn test_llm_error_display() {
        let err = LlmError::Api {
            status: 429,
            body: "rate limited".to_string(),
        };
        assert_eq!(err.to_string(), "API error 429: rate limited");

        let err2 = LlmError::RetryExhausted { attempts: 4 };
        assert_eq!(err2.to_string(), "retry exhausted after 4 attempts");
    }
}
