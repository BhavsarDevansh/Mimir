use serde::{Deserialize, Serialize};

/// Request body for chat endpoints.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ChatRequest {
    /// Existing session id; if omitted a new session is created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// User message content.
    pub message: String,
    /// Optional model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional personality preset override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality_preset: Option<String>,
    /// When true, skip all persistence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incognito: Option<bool>,
}

/// Response body for the non-streaming chat endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatResponse {
    pub session_id: String,
    pub response: String,
    pub usage: Usage,
}

/// Token usage statistics for a completion request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Response body for the status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusResponse {
    pub version: String,
    pub uptime_seconds: u64,
    pub queue_depth_user: usize,
    pub queue_depth_system: usize,
    pub worker_threads: u8,
    pub endpoint: String,
    pub model: String,
    pub config_path: Option<String>,
    pub config_exists: bool,
    pub llm_reachable: bool,
    pub context_window: Option<u32>,
    pub memory_path: String,
    pub memory_exists: bool,
    pub memory_chars: usize,
    pub memory_limit: usize,
    pub memory_usage_pct: f64,
}

/// An item yielded by the client-side SSE stream parser.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamItem {
    Text(String),
    Usage(Usage),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_request_roundtrip() {
        let req = ChatRequest {
            session_id: Some("sess-123".to_string()),
            message: "hello".to_string(),
            model: Some("gpt-4o".to_string()),
            personality_preset: Some("coder".to_string()),
            incognito: Some(true),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ChatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn test_chat_request_omits_nulls() {
        let req = ChatRequest {
            session_id: None,
            message: "hi".to_string(),
            model: None,
            personality_preset: None,
            incognito: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("model"));
        assert!(!json.contains("personality_preset"));
        assert!(!json.contains("incognito"));
        assert!(json.contains("message"));
    }

    #[test]
    fn test_chat_response_roundtrip() {
        let resp = ChatResponse {
            session_id: "sess-456".to_string(),
            response: "world".to_string(),
            usage: Usage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ChatResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn test_status_response_roundtrip() {
        let status = StatusResponse {
            version: "0.13.0".to_string(),
            uptime_seconds: 42,
            queue_depth_user: 1,
            queue_depth_system: 2,
            worker_threads: 4,
            endpoint: "http://localhost:8080".to_string(),
            model: "gpt-4o".to_string(),
            config_path: Some("/cfg".to_string()),
            config_exists: true,
            llm_reachable: true,
            context_window: Some(128_000),
            memory_path: "/mem".to_string(),
            memory_exists: true,
            memory_chars: 100,
            memory_limit: 10_000,
            memory_usage_pct: 1.0,
        };
        let json = serde_json::to_string(&status).unwrap();
        let back: StatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(status, back);
    }

    #[test]
    fn test_usage_default() {
        let u = Usage::default();
        assert_eq!(u.prompt_tokens, 0);
        assert_eq!(u.completion_tokens, 0);
        assert_eq!(u.total_tokens, 0);
    }

    #[test]
    fn test_stream_item_equality() {
        let item = StreamItem::Text("hello".to_string());
        assert_eq!(item, StreamItem::Text("hello".to_string()));
        assert_ne!(item, StreamItem::Usage(Usage::default()));
    }
}
