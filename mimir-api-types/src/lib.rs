use serde::{Deserialize, Serialize};

/// Lightweight summary of a conversation session.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SessionSummary {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub preview: Option<String>,
}

/// A single message in a session history response.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Response body for fetching session messages.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SessionMessagesResponse {
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
}

/// Information about a tool call executed during a chat completion.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolCallInfo {
    /// Snake_case tool identifier.
    pub name: String,
    /// Human-readable display name (e.g. "Get Current Time").
    pub display_name: String,
    /// Compact result summary (single line, truncated to ~80 chars).
    pub result: String,
}

impl ToolCallInfo {
    /// Maximum length for the result summary.
    pub const MAX_RESULT_LEN: usize = 80;

    /// Truncate a result string to a single line of at most MAX_RESULT_LEN characters.
    pub fn truncate_result(result: &str) -> String {
        let first_line = result.lines().next().unwrap_or(result);
        if first_line.chars().count() > Self::MAX_RESULT_LEN {
            let truncated: String = first_line.chars().take(Self::MAX_RESULT_LEN).collect();
            format!("{truncated}…")
        } else {
            first_line.to_string()
        }
    }
}

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
    /// Tool calls executed during this completion (empty for responses without tool use).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallInfo>,
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
    pub memory_exists: bool,
    pub memory_chars: usize,
    pub memory_limit: usize,
    pub memory_usage_pct: f64,
}

/// Summary of a single optimization run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizationRunSummary {
    pub run_id: i64,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

/// Response body for the KG optimization status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizationStatusResponse {
    pub job_id: String,
    pub priority: String,
    pub schedule: Option<String>,
    pub next_run_at: Option<String>,
    pub last_run: Option<OptimizationRunSummary>,
}

/// Response body for the KG optimization run-now endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizationRunNowResponse {
    pub run_id: i64,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

/// An item yielded by the client-side SSE stream parser.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamItem {
    Text(String),
    Usage(Usage),
    ToolCall(ToolCallInfo),
    /// Server-assigned session ID (emitted once at stream start).
    SessionId(String),
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
            tool_calls: vec![],
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
        assert_ne!(item, StreamItem::SessionId("sess-1".to_string()));
    }

    #[test]
    fn test_tool_call_info_roundtrip() {
        let info = ToolCallInfo {
            name: "get_current_time".to_string(),
            display_name: "Get Current Time".to_string(),
            result: "2025-05-30T12:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ToolCallInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn test_tool_call_info_truncate_short() {
        let result = ToolCallInfo::truncate_result("ok");
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_tool_call_info_truncate_long() {
        let long = "a".repeat(100);
        let result = ToolCallInfo::truncate_result(&long);
        assert_eq!(result.chars().count(), 81); // 80 chars + ellipsis
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_tool_call_info_truncate_multibyte() {
        // Multi-byte UTF-8 (emoji) should not panic on byte-slice truncation.
        let emoji_result = "🎉".repeat(100); // 100 chars, 400 bytes
        let result = ToolCallInfo::truncate_result(&emoji_result);
        assert_eq!(result.chars().count(), 81); // 80 emoji + ellipsis
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_tool_call_info_truncate_multiline() {
        let result = ToolCallInfo::truncate_result(
            "line1
line2",
        );
        assert_eq!(result, "line1");
    }

    #[test]
    fn test_chat_response_with_tool_calls() {
        let resp = ChatResponse {
            session_id: "sess-1".to_string(),
            response: "done".to_string(),
            usage: Usage::default(),
            tool_calls: vec![ToolCallInfo {
                name: "echo".to_string(),
                display_name: "Echo".to_string(),
                result: "hello".to_string(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ChatResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_calls.len(), 1);
        assert_eq!(back.tool_calls[0].name, "echo");
    }
}
