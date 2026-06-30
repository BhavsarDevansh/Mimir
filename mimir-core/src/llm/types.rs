use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A chat completion request compatible with the OpenAI API.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none", rename = "stream_options")]
    pub stream_options: Option<serde_json::Value>,
}

/// A function call inside a tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FunctionCall {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// A tool call issued by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ToolCall {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub call_type: String,
    #[serde(default)]
    pub function: FunctionCall,
}

fn deserialize_null_to_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    #[serde(default, deserialize_with = "deserialize_null_to_empty_string")]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Model metadata returned by the `/models` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: Option<String>,
    #[serde(default)]
    pub created: Option<u64>,
    pub owned_by: Option<String>,
    /// Provider-specific context window size (tokens).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

/// A list of models returned by the `/models` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelList {
    pub object: Option<String>,
    pub data: Vec<ModelInfo>,
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
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
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
    pub usage: Option<Usage>,
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
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// An item yielded by the usage-aware streaming chat method.
#[derive(Debug, Clone)]
pub enum StreamItem {
    Text(String),
    Usage(Usage),
    /// Partial tool-call deltas from a streaming response.
    ToolCalls(Vec<ToolCall>),
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

    /// The worker pool queues are full.
    #[error("worker pool queue full")]
    QueueFull,

    /// The HTTP client or worker pool could not be constructed at startup.
    #[error("client build error: {0}")]
    ClientBuild(String),
}

impl ChatRequest {
    /// Create a new chat request with the given model and messages.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: None,
            max_tokens: None,
            temperature: None,
            stream: false,
            stream_options: None,
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

    /// Set the tools to include in the request.
    pub fn with_tools(mut self, tools: Vec<serde_json::Value>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Enable or disable streaming.
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// Set stream options (e.g. `{"include_usage": true}`).
    pub fn with_stream_options(mut self, options: serde_json::Value) -> Self {
        self.stream_options = Some(options);
        self
    }
}

impl Message {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a tool result message.
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A job dispatched to the LLM worker pool.
pub enum Job {
    /// Non-streaming chat completion.
    Chat {
        /// Conversation messages.
        messages: Vec<Message>,
        /// Optional tools to include in the request.
        tools: Option<Vec<serde_json::Value>>,
        /// Channel to send the result back.
        respond: tokio::sync::oneshot::Sender<Result<(Message, Usage), LlmError>>,
    },
    /// Streaming chat completion.
    ChatStream {
        /// Conversation messages.
        messages: Vec<Message>,
        /// Optional tools to include in the request.
        tools: Option<Vec<serde_json::Value>>,
        /// Channel to stream items back.
        respond: tokio::sync::mpsc::Sender<Result<StreamItem, LlmError>>,
    },
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
    fn test_chat_request_serializes_tools() {
        let req = ChatRequest::new("gpt-4o", vec![Message::user("hello")]).with_tools(vec![
            serde_json::json!({"type": "function", "function": {"name": "echo"}}),
        ]);

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"echo\""));
    }

    #[test]
    fn test_chat_request_skips_none_tools() {
        let req = ChatRequest::new("gpt-4o", vec![Message::user("hello")]);
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"tools\""));
    }

    #[test]
    fn test_chat_request_serializes_stream_options() {
        let req = ChatRequest::new("gpt-4o", vec![Message::user("hello")])
            .with_stream(true)
            .with_stream_options(serde_json::json!({"include_usage": true}));

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"stream_options\""));
        assert!(json.contains("\"include_usage\":true"));
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
    fn test_stream_chunk_parses_with_usage() {
        let json = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion.chunk",
            "created": 1677652288,
            "model": "gpt-4o",
            "choices": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;

        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.choices.is_empty());
        let usage = chunk.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
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
