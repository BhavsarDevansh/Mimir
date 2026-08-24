use serde::{Deserialize, Serialize};

/// Lightweight summary of a conversation session.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SessionSummary {
    pub session_id: i64,
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
    pub session_id: i64,
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

/// Metadata emitted at the start of a tool call, before the result is known.
///
/// Sent via the `tool_call_start` SSE event so the client can show a
/// "working" indicator while the tool executes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolCallStartInfo {
    /// Snake_case tool identifier.
    pub name: String,
    /// Human-readable display name (e.g. "Get Current Time").
    pub display_name: String,
}

/// Request body for chat endpoints.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ChatRequest {
    /// Existing session id; if omitted a new session is created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
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
    pub session_id: i64,
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
    /// Older daemons omit this field; default to 0 so a new CLI keeps
    /// working against an older daemon.
    #[serde(default)]
    pub hook_queue_depth: usize,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response body for the KG optimization status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizationStatusResponse {
    pub job_id: String,
    pub priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<OptimizationRunSummary>,
}

/// Response body for the KG optimization run-now endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizationRunNowResponse {
    pub run_id: i64,
    pub status: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    /// Tool call has started executing (result not yet available).
    ToolCallStart(ToolCallStartInfo),
}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider surface (issue #388)
// ---------------------------------------------------------------------------

/// A message in an OpenAI chat completion request or response.
///
/// `content` is `None` for assistant messages that carry only tool calls
/// (OpenAI serialises that as JSON `null`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// A function call inside an OpenAI tool call.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct OpenAiFunctionCall {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// A tool call in an OpenAI response message.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OpenAiFunctionCall,
}

/// A streamed tool-call delta.
///
/// Mirrors the OpenAI streaming shape: the first delta for a call carries
/// `index`, `id`, `type`, and the function name; later deltas carry only
/// `index` and argument fragments.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiToolCallDelta {
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAiFunctionCall>,
}

/// Streaming options for an OpenAI chat completion request.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct OpenAiStreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

/// Request body for the OpenAI-compatible chat completions endpoint.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<OpenAiStreamOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    /// Conversation key that resumes one persistent session in the central
    /// profile. Absent or blank values key the fixed `default` session —
    /// every request persists and learns, there is no incognito path on this
    /// surface (issue #473).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// Token usage for an OpenAI chat completion.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// The assistant message in an OpenAI chat completion choice.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiResponseMessage {
    pub role: String,
    // `content` is always serialised: tool-call responses carry an explicit
    // `null`, matching the OpenAI wire shape (PR #466 review).
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAiToolCall>,
}

/// A single choice in an OpenAI chat completion response.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiChoice {
    pub index: u32,
    pub message: OpenAiResponseMessage,
    pub finish_reason: String,
}

/// Response body for the non-streaming OpenAI chat completions endpoint.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAiChoice>,
    pub usage: OpenAiUsage,
}

/// The delta content in a streaming chunk.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct OpenAiDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAiToolCallDelta>,
}

/// A single choice within a streaming chunk.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiStreamChoice {
    pub index: u32,
    pub delta: OpenAiDelta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// A single SSE chunk of a streaming OpenAI chat completion.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiStreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAiStreamChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAiUsage>,
}

/// A single model entry in the OpenAI models list.
///
/// `description` is a Mimir extension (personality preset descriptions);
/// `created` is always `0` because presets have no upstream creation time.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiModel {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The OpenAI models list response.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiModelList {
    pub object: String,
    pub data: Vec<OpenAiModel>,
}

/// A single OpenAI-shaped error.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// OpenAI-shaped error response body.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OpenAiErrorBody {
    pub error: OpenAiError,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Round-trip helper: serialise then deserialise must yield an equal value.
    fn roundtrip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialise");
        serde_json::from_str(&json).expect("deserialise")
    }

    // Macro: declare a round-trip test for one struct, covering both
    // populated and `Option::None` (skip-serialising) forms.
    macro_rules! roundtrip_tests {
        ($name:ident, full: $full:expr, sparse: $sparse:expr, sparse_skips: [$($skip:literal),* $(,)?]) => {
            #[test]
            fn $name() {
                assert_eq!(roundtrip(&$full), $full);
                assert_eq!(roundtrip(&$sparse), $sparse);
                let json = serde_json::to_string(&$sparse).expect("serialise sparse");
                let obj = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
                    .expect("parse sparse json object");
                $(
                    assert!(
                        !obj.contains_key($skip),
                        "sparse form should not serialise `{}` (got: {json})",
                        $skip,
                    );
                )*
                // Keep `json` and `obj` consumed even when `sparse_skips` is empty
                // so the macro never emits unused-variable warnings.
                let _ = (&json, &obj);
            }
        };
    }

    #[test]
    fn test_chat_request_roundtrip() {
        let req = ChatRequest {
            session_id: Some(123),
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
            session_id: 456,
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
            hook_queue_depth: 3,
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
    fn test_status_response_accepts_older_payload_without_hook_queue_depth() {
        // A new CLI must keep deserialising `/status` responses from older
        // daemons that predate `hook_queue_depth` (PR #442 review).
        let json = r#"{
            "version": "0.130.6",
            "uptime_seconds": 42,
            "queue_depth_user": 1,
            "queue_depth_system": 2,
            "worker_threads": 4,
            "endpoint": "http://localhost:8080",
            "model": "gpt-4o",
            "config_path": null,
            "config_exists": true,
            "llm_reachable": true,
            "context_window": null,
            "memory_exists": true,
            "memory_chars": 100,
            "memory_limit": 10000,
            "memory_usage_pct": 1.0
        }"#;
        let back: StatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(back.hook_queue_depth, 0);
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
        assert_ne!(
            item,
            StreamItem::ToolCallStart(ToolCallStartInfo {
                name: "echo".to_string(),
                display_name: "Echo".to_string(),
            })
        );
    }

    #[test]
    fn test_tool_call_start_info_roundtrip() {
        let info = ToolCallStartInfo {
            name: "get_current_time".to_string(),
            display_name: "Get Current Time".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ToolCallStartInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
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
            session_id: 1,
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

    #[test]
    fn optimization_run_summary_roundtrip() {
        let summary = OptimizationRunSummary {
            run_id: 1,
            status: "completed".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: Some("2020-01-01T00:05:00Z".to_string()),
            error: None,
        };
        assert_eq!(roundtrip(&summary), summary);
    }

    roundtrip_tests!(
        optimization_run_summary_sparse,
        full: OptimizationRunSummary {
            run_id: 1,
            status: "completed".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: Some("2020-01-01T00:05:00Z".to_string()),
            error: Some("boom".to_string()),
        },
        sparse: OptimizationRunSummary {
            run_id: 2,
            status: "running".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: None,
            error: None,
        },
        sparse_skips: ["finished_at", "error"]
    );

    roundtrip_tests!(
        optimization_status_response,
        full: OptimizationStatusResponse {
            job_id: "kg-optimization".to_string(),
            priority: "low".to_string(),
            schedule: Some("daily".to_string()),
            next_run_at: Some("2020-01-02T00:00:00Z".to_string()),
            last_run: Some(OptimizationRunSummary {
                run_id: 1,
                status: "completed".to_string(),
                started_at: "2020-01-01T00:00:00Z".to_string(),
                finished_at: Some("2020-01-01T00:05:00Z".to_string()),
                error: None,
            }),
        },
        sparse: OptimizationStatusResponse {
            job_id: "kg-optimization".to_string(),
            priority: "low".to_string(),
            schedule: None,
            next_run_at: None,
            last_run: None,
        },
        sparse_skips: ["schedule", "next_run_at", "last_run"]
    );

    roundtrip_tests!(
        optimization_run_now_response,
        full: OptimizationRunNowResponse {
            run_id: 2,
            status: "running".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: Some("2020-01-01T00:05:00Z".to_string()),
            error: Some("boom".to_string()),
        },
        sparse: OptimizationRunNowResponse {
            run_id: 2,
            status: "running".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: None,
            error: None,
        },
        sparse_skips: ["finished_at", "error"]
    );

    #[test]
    fn stream_item_variants_equality() {
        let usage = Usage {
            prompt_tokens: 5,
            completion_tokens: 7,
            total_tokens: 12,
        };
        assert_eq!(StreamItem::Usage(usage.clone()), StreamItem::Usage(usage));
        let tool = ToolCallInfo {
            name: "echo".to_string(),
            display_name: "Echo".to_string(),
            result: "hi".to_string(),
        };
        assert_eq!(
            StreamItem::ToolCall(tool.clone()),
            StreamItem::ToolCall(tool)
        );
        assert_eq!(
            StreamItem::SessionId("s".to_string()),
            StreamItem::SessionId("s".to_string())
        );
    }

    #[test]
    fn optimization_run_summary_with_error_roundtrip() {
        let summary = OptimizationRunSummary {
            run_id: 3,
            status: "failed".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: None,
            error: Some("disk full".to_string()),
        };
        assert_eq!(roundtrip(&summary), summary);
    }

    // -- OpenAI-compatible provider surface (issue #388) --

    #[test]
    fn openai_chat_request_roundtrip() {
        let req = OpenAiChatRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                OpenAiChatMessage {
                    role: "user".to_string(),
                    content: Some("hello".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAiChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![OpenAiToolCall {
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: OpenAiFunctionCall {
                            name: "get_weather".to_string(),
                            arguments: "{\"location\":\"London\"}".to_string(),
                        },
                    }]),
                    tool_call_id: None,
                },
                OpenAiChatMessage {
                    role: "tool".to_string(),
                    content: Some("sunny".to_string()),
                    tool_calls: None,
                    tool_call_id: Some("call_1".to_string()),
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(256),
            max_completion_tokens: None,
            stream: true,
            stream_options: Some(OpenAiStreamOptions {
                include_usage: true,
            }),
            tools: Some(vec![serde_json::json!({
                "type": "function",
                "function": {"name": "get_weather", "parameters": {"type": "object"}}
            })]),
            user: Some("phone".to_string()),
        };
        assert_eq!(roundtrip(&req), req);
    }

    #[test]
    fn openai_chat_request_sparse_omits_optional_fields() {
        let req = OpenAiChatRequest {
            model: "transparent".to_string(),
            messages: vec![OpenAiChatMessage {
                role: "user".to_string(),
                content: Some("hi".to_string()),
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            stream: false,
            stream_options: None,
            tools: None,
            user: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        for field in [
            "\"temperature\"",
            "\"max_tokens\"",
            "\"max_completion_tokens\"",
            "\"stream_options\"",
            "\"tools\"",
            "\"user\":",
        ] {
            assert!(
                !json.contains(field),
                "sparse request serialised `{field}`: {json}"
            );
        }
        assert_eq!(roundtrip(&req), req);
    }

    #[test]
    fn openai_chat_response_roundtrip() {
        let resp = OpenAiChatResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1_700_000_000,
            model: "gpt-4o".to_string(),
            choices: vec![OpenAiChoice {
                index: 0,
                message: OpenAiResponseMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello!".to_string()),
                    tool_calls: Vec::new(),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: OpenAiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn openai_tool_calls_response_serializes_null_content() {
        let resp = OpenAiChatResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1_700_000_000,
            model: "gpt-4o".to_string(),
            choices: vec![OpenAiChoice {
                index: 0,
                message: OpenAiResponseMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: vec![OpenAiToolCall {
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: OpenAiFunctionCall {
                            name: "get_weather".to_string(),
                            arguments: "{\"location\":\"London\"}".to_string(),
                        },
                    }],
                },
                finish_reason: "tool_calls".to_string(),
            }],
            usage: OpenAiUsage::default(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let message = &value["choices"][0]["message"];
        assert!(
            message.get("content").is_some(),
            "tool-call responses must serialise the content key: {json}"
        );
        assert_eq!(message["content"], serde_json::Value::Null);
        assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            value["choices"][0]["message"]["tool_calls"][0]["type"],
            "function"
        );
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn openai_stream_chunk_roundtrip() {
        let chunk = OpenAiStreamChunk {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1_700_000_000,
            model: "gpt-4o".to_string(),
            choices: vec![OpenAiStreamChoice {
                index: 0,
                delta: OpenAiDelta {
                    role: Some("assistant".to_string()),
                    content: Some("Hello".to_string()),
                    tool_calls: Vec::new(),
                },
                finish_reason: None,
            }],
            usage: None,
        };
        assert_eq!(roundtrip(&chunk), chunk);
    }

    #[test]
    fn openai_stream_chunk_tool_call_delta_shape() {
        let chunk = OpenAiStreamChunk {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1_700_000_000,
            model: "gpt-4o".to_string(),
            choices: vec![OpenAiStreamChoice {
                index: 0,
                delta: OpenAiDelta {
                    role: None,
                    content: None,
                    tool_calls: vec![OpenAiToolCallDelta {
                        index: 0,
                        id: Some("call_1".to_string()),
                        call_type: Some("function".to_string()),
                        function: Some(OpenAiFunctionCall {
                            name: "get_weather".to_string(),
                            arguments: "".to_string(),
                        }),
                    }],
                },
                finish_reason: None,
            }],
            usage: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let delta = &value["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(delta["index"], 0);
        assert_eq!(delta["id"], "call_1");
        assert_eq!(delta["type"], "function");
        assert_eq!(delta["function"]["name"], "get_weather");
        assert_eq!(roundtrip(&chunk), chunk);
    }

    #[test]
    fn openai_usage_chunk_has_empty_choices() {
        let chunk = OpenAiStreamChunk {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1_700_000_000,
            model: "gpt-4o".to_string(),
            choices: Vec::new(),
            usage: Some(OpenAiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["choices"], serde_json::json!([]));
        assert_eq!(value["usage"]["total_tokens"], 15);
        assert_eq!(roundtrip(&chunk), chunk);
    }

    #[test]
    fn openai_model_list_roundtrip() {
        let list = OpenAiModelList {
            object: "list".to_string(),
            data: vec![
                OpenAiModel {
                    id: "transparent".to_string(),
                    object: "model".to_string(),
                    created: 0,
                    owned_by: "mimir".to_string(),
                    description: Some("Warm, efficient, shows its work".to_string()),
                },
                OpenAiModel {
                    id: "custom".to_string(),
                    object: "model".to_string(),
                    created: 0,
                    owned_by: "mimir".to_string(),
                    description: None,
                },
            ],
        };
        assert_eq!(roundtrip(&list), list);
    }

    #[test]
    fn openai_error_body_roundtrip() {
        let body = OpenAiErrorBody {
            error: OpenAiError {
                message: "server busy, try again later".to_string(),
                error_type: "server_error".to_string(),
                param: None,
                code: Some("queue_full".to_string()),
            },
        };
        let json = serde_json::to_string(&body).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["error"]["type"], "server_error");
        assert_eq!(value["error"]["code"], "queue_full");
        assert_eq!(roundtrip(&body), body);
    }
}
