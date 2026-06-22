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

// ---------------------------------------------------------------------------
// Knowledge Graph — CLI types
// ---------------------------------------------------------------------------

/// Request to query facts for an entity.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FactQueryParams {
    pub entity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A single fact in query results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactRow {
    pub id: i32,
    pub subject: String,
    pub predicate: String,
    pub object: Option<String>,
    pub confidence: f32,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    pub inferred: bool,
}

/// Response for fact queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactQueryResponse {
    pub total: i64,
    pub offset: u32,
    pub limit: u32,
    pub facts: Vec<FactRow>,
}

/// Source attached to a fact (detail view).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceRow {
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_reference: Option<String>,
    pub extracted_at: String,
}

/// Dependency edge for a fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyRow {
    pub relation_type: String,
    pub parent_fact_id: i32,
    pub child_fact_id: i32,
}

/// Audit log entry for a fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRow {
    pub audit_id: i32,
    pub fact_id: i32,
    pub change_type: String,
    pub entity_name: Option<String>,
    pub predicate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
    pub changed_at: String,
    pub changed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Detailed view of a single fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactDetailResponse {
    pub fact: FactRow,
    pub sources: Vec<SourceRow>,
    pub dependencies: Vec<DependencyRow>,
    pub audit_log: Vec<AuditRow>,
}

/// Request to edit a fact.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FactEditRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_literal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Response after editing a fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactEditResponse {
    pub fact: FactRow,
}

/// Request to browse the knowledge graph.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BrowseRequest {
    pub entity: String,
    #[serde(default = "default_browse_depth")]
    pub depth: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

fn default_browse_depth() -> u32 {
    2
}

/// A single edge in a browse traversal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowseEdge {
    pub depth: u32,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
}

/// Response for browse queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowseResponse {
    pub total_edges: usize,
    pub offset: u32,
    pub limit: u32,
    pub edges: Vec<BrowseEdge>,
}

/// A category in the knowledge graph taxonomy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i32>,
    pub memory_weight: Option<f32>,
}

/// A category with its child categories and fact count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryDetailResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i32>,
    pub memory_weight: Option<f32>,
    pub fact_count: i64,
    pub children: Vec<CategoryResponse>,
}
/// Request to generate a profile.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ProfileRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
}

/// A group of facts in a profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileGroup {
    pub category: String,
    pub facts: Vec<FactRow>,
}

/// Response for profile queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileResponse {
    pub entity_name: String,
    pub groups: Vec<ProfileGroup>,
}

/// Request to query the audit log.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AuditQueryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Response for audit log queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditQueryResponse {
    pub total: i64,
    pub offset: u32,
    pub limit: u32,
    pub entries: Vec<AuditRow>,
}

/// Request to forget facts.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ForgetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub yes: bool,
    #[serde(default)]
    pub confirm_sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_phrase: Option<String>,
    #[serde(default)]
    pub archive: bool,
}

/// Response after forgetting facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForgetResponse {
    pub forgotten_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

/// Request to restore facts from trash.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RestoreRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash_id: Option<i32>,
    #[serde(default)]
    pub all: bool,
}

/// Response after restoring facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestoreResponse {
    pub restored_count: usize,
}

/// A single row in the trash list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrashRow {
    pub trash_id: i32,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub deleted_at: String,
    pub expires_at: String,
}

/// Response for trash list queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrashListResponse {
    pub total: i64,
    pub offset: u32,
    pub limit: u32,
    pub items: Vec<TrashRow>,
}

// ---------------------------------------------------------------------------
// Knowledge Graph — pending sensitive-fact confirmation (issue #141)
// ---------------------------------------------------------------------------

/// A single pending sensitive fact awaiting user confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingFactRow {
    pub fact_id: i32,
    pub subject: String,
    pub predicate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    pub created_at: String,
}

/// Response body for `GET /kb/pending`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingListResponse {
    pub total: usize,
    pub facts: Vec<PendingFactRow>,
}

/// Response body for `POST /kb/facts/{id}/confirm`.
///
/// Wraps the updated fact as a [`FactRow`], consistent with the edit endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfirmFactResponse {
    pub fact: FactRow,
}

/// Request body for `POST /kb/facts/{id}/reject`.
///
/// All fields optional: an empty POST body is valid and yields a `204 No
/// Content`. A `reason`, if supplied, is written to the audit log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RejectFactRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn sample_fact_row() -> FactRow {
        FactRow {
            id: 7,
            subject: "Alice".to_string(),
            predicate: "lives_in".to_string(),
            object: Some("London".to_string()),
            confidence: 0.9,
            status: "active".to_string(),
            valid_from: Some("2020-01-01T00:00:00Z".to_string()),
            valid_until: None,
            inferred: false,
        }
    }

    roundtrip_tests!(
        fact_query_params,
        full: FactQueryParams {
            entity: "Alice".to_string(),
            predicate: Some("lives_in".to_string()),
            min_confidence: Some(0.5),
            offset: Some(10),
            limit: Some(20),
        },
        sparse: FactQueryParams {
            entity: "Bob".to_string(),
            predicate: None,
            min_confidence: None,
            offset: None,
            limit: None,
        },
        sparse_skips: ["predicate", "min_confidence", "offset", "limit"]
    );

    #[test]
    fn fact_row_roundtrip() {
        let row = sample_fact_row();
        assert_eq!(roundtrip(&row), row);
    }

    #[test]
    fn fact_query_response_roundtrip() {
        let resp = FactQueryResponse {
            total: 2,
            offset: 0,
            limit: 10,
            facts: vec![sample_fact_row()],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        source_row,
        full: SourceRow {
            source_type: "chat".to_string(),
            connector_id: Some("cli".to_string()),
            raw_reference: Some("ref-1".to_string()),
            extracted_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse: SourceRow {
            source_type: "chat".to_string(),
            connector_id: None,
            raw_reference: None,
            extracted_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse_skips: ["connector_id", "raw_reference"]
    );

    #[test]
    fn dependency_row_roundtrip() {
        let row = DependencyRow {
            relation_type: "transitive".to_string(),
            parent_fact_id: 1,
            child_fact_id: 2,
        };
        assert_eq!(roundtrip(&row), row);
    }

    roundtrip_tests!(
        audit_row,
        full: AuditRow {
            audit_id: 9,
            fact_id: 1,
            change_type: "status_change".to_string(),
            entity_name: Some("Alice".to_string()),
            predicate_name: Some("lives_in".to_string()),
            old_value: Some("Paris".to_string()),
            new_value: Some("London".to_string()),
            changed_at: "2020-01-01T00:00:00Z".to_string(),
            changed_by: Some("user".to_string()),
            reason: Some("correction".to_string()),
        },
        sparse: AuditRow {
            audit_id: 9,
            fact_id: 1,
            change_type: "status_change".to_string(),
            entity_name: None,
            predicate_name: None,
            old_value: None,
            new_value: None,
            changed_at: "2020-01-01T00:00:00Z".to_string(),
            changed_by: None,
            reason: None,
        },
        sparse_skips: [
            "old_value",
            "new_value",
            "reason"
        ]
    );

    #[test]
    fn fact_detail_response_roundtrip() {
        let detail = FactDetailResponse {
            fact: sample_fact_row(),
            sources: vec![SourceRow {
                source_type: "chat".to_string(),
                connector_id: None,
                raw_reference: None,
                extracted_at: "2020-01-01T00:00:00Z".to_string(),
            }],
            dependencies: vec![],
            audit_log: vec![],
        };
        assert_eq!(roundtrip(&detail), detail);
    }

    roundtrip_tests!(
        fact_edit_request,
        full: FactEditRequest {
            confidence: Some(0.8),
            valid_from: Some("2020-01-01T00:00:00Z".to_string()),
            valid_until: None,
            object_literal: Some("London".to_string()),
            status: Some("active".to_string()),
        },
        sparse: FactEditRequest {
            confidence: None,
            valid_from: None,
            valid_until: None,
            object_literal: None,
            status: None,
        },
        sparse_skips: [
            "confidence",
            "valid_from",
            "valid_until",
            "object_literal",
            "status"
        ]
    );

    #[test]
    fn fact_edit_response_roundtrip() {
        let resp = FactEditResponse {
            fact: sample_fact_row(),
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn browse_request_default_depth_is_applied() {
        let json = r#"{"entity":"Alice"}"#;
        let parsed: BrowseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.entity, "Alice");
        assert_eq!(parsed.depth, 2);
        assert_eq!(parsed.offset, None);
        assert_eq!(parsed.limit, None);
    }

    roundtrip_tests!(
        browse_request,
        full: BrowseRequest {
            entity: "Alice".to_string(),
            depth: 3,
            offset: Some(0),
            limit: Some(10),
        },
        sparse: BrowseRequest {
            entity: "Alice".to_string(),
            depth: 2,
            offset: None,
            limit: None,
        },
        sparse_skips: ["offset", "limit"]
    );

    #[test]
    fn browse_edge_and_response_roundtrip() {
        let edge = BrowseEdge {
            depth: 1,
            subject: "Alice".to_string(),
            predicate: "lives_in".to_string(),
            object: "London".to_string(),
            confidence: 0.9,
        };
        assert_eq!(roundtrip(&edge), edge);

        let resp = BrowseResponse {
            total_edges: 1,
            offset: 0,
            limit: 10,
            edges: vec![edge],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        category_response,
        full: CategoryResponse {
            id: 1,
            name: "people".to_string(),
            description: Some("Humans".to_string()),
            parent_id: Some(0),
            memory_weight: Some(0.8),
        },
        sparse: CategoryResponse {
            id: 1,
            name: "people".to_string(),
            description: None,
            parent_id: None,
            memory_weight: None,
        },
        sparse_skips: []
    );

    #[test]
    fn category_detail_response_roundtrip() {
        let detail = CategoryDetailResponse {
            id: 1,
            name: "people".to_string(),
            description: Some("Humans".to_string()),
            parent_id: None,
            memory_weight: Some(0.8),
            fact_count: 5,
            children: vec![CategoryResponse {
                id: 2,
                name: "friends".to_string(),
                description: None,
                parent_id: Some(1),
                memory_weight: None,
            }],
        };
        assert_eq!(roundtrip(&detail), detail);
    }

    roundtrip_tests!(
        profile_request,
        full: ProfileRequest {
            entity: Some("Alice".to_string()),
        },
        sparse: ProfileRequest { entity: None },
        sparse_skips: ["entity"]
    );

    #[test]
    fn profile_response_roundtrip() {
        let resp = ProfileResponse {
            entity_name: "Alice".to_string(),
            groups: vec![ProfileGroup {
                category: "personal".to_string(),
                facts: vec![sample_fact_row()],
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        audit_query_request,
        full: AuditQueryRequest {
            entity: Some("Alice".to_string()),
            predicate: Some("lives_in".to_string()),
            from: Some("2020-01-01T00:00:00Z".to_string()),
            to: Some("2021-01-01T00:00:00Z".to_string()),
            change_type: Some("status_change".to_string()),
            offset: Some(0),
            limit: Some(10),
        },
        sparse: AuditQueryRequest {
            entity: None,
            predicate: None,
            from: None,
            to: None,
            change_type: None,
            offset: None,
            limit: None,
        },
        sparse_skips: [
            "entity",
            "predicate",
            "from",
            "to",
            "change_type",
            "offset",
            "limit"
        ]
    );

    #[test]
    fn audit_query_response_roundtrip() {
        let resp = AuditQueryResponse {
            total: 1,
            offset: 0,
            limit: 10,
            entries: vec![AuditRow {
                audit_id: 1,
                fact_id: 1,
                change_type: "status_change".to_string(),
                entity_name: Some("Alice".to_string()),
                predicate_name: Some("lives_in".to_string()),
                old_value: None,
                new_value: Some("London".to_string()),
                changed_at: "2020-01-01T00:00:00Z".to_string(),
                changed_by: None,
                reason: None,
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn forget_request_defaults() {
        let parsed: ForgetRequest = serde_json::from_str("{}").unwrap();
        assert!(!parsed.all);
        assert!(!parsed.yes);
        assert!(!parsed.confirm_sensitive);
        assert!(!parsed.archive);
        assert_eq!(parsed.fact_id, None);
        assert_eq!(parsed.confirmation_phrase, None);
    }

    roundtrip_tests!(
        forget_request,
        full: ForgetRequest {
            fact_id: Some(42),
            predicate: Some("lives_in".to_string()),
            subject: Some("Alice".to_string()),
            entity: Some("Alice".to_string()),
            source: Some("chat".to_string()),
            from: Some("2020-01-01T00:00:00Z".to_string()),
            to: Some("2021-01-01T00:00:00Z".to_string()),
            all: false,
            yes: true,
            confirm_sensitive: true,
            confirmation_phrase: Some("I am sure".to_string()),
            archive: true,
        },
        sparse: ForgetRequest {
            fact_id: None,
            predicate: None,
            subject: None,
            entity: None,
            source: None,
            from: None,
            to: None,
            all: false,
            yes: false,
            confirm_sensitive: false,
            confirmation_phrase: None,
            archive: false,
        },
        sparse_skips: [
            "fact_id",
            "predicate",
            "subject",
            "entity",
            "source",
            "from",
            "to",
            "confirmation_phrase"
        ]
    );

    roundtrip_tests!(
        forget_response,
        full: ForgetResponse {
            forgotten_count: 5,
            backup_path: Some("/tmp/backup.json".to_string()),
        },
        sparse: ForgetResponse {
            forgotten_count: 0,
            backup_path: None,
        },
        sparse_skips: ["backup_path"]
    );

    roundtrip_tests!(
        restore_request,
        full: RestoreRequest {
            trash_id: Some(7),
            all: false,
        },
        sparse: RestoreRequest {
            trash_id: None,
            all: true,
        },
        sparse_skips: ["trash_id"]
    );

    #[test]
    fn restore_response_roundtrip() {
        let resp = RestoreResponse { restored_count: 3 };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        trash_row,
        full: TrashRow {
            trash_id: 1,
            subject: Some("Alice".to_string()),
            predicate: Some("lives_in".to_string()),
            object: Some("London".to_string()),
            deleted_at: "2020-01-01T00:00:00Z".to_string(),
            expires_at: "2021-01-01T00:00:00Z".to_string(),
        },
        sparse: TrashRow {
            trash_id: 1,
            subject: None,
            predicate: None,
            object: None,
            deleted_at: "2020-01-01T00:00:00Z".to_string(),
            expires_at: "2021-01-01T00:00:00Z".to_string(),
        },
        sparse_skips: []
    );

    #[test]
    fn trash_list_response_roundtrip() {
        let resp = TrashListResponse {
            total: 1,
            offset: 0,
            limit: 10,
            items: vec![TrashRow {
                trash_id: 1,
                subject: Some("Alice".to_string()),
                predicate: None,
                object: None,
                deleted_at: "2020-01-01T00:00:00Z".to_string(),
                expires_at: "2021-01-01T00:00:00Z".to_string(),
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
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
        sparse_skips: []
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
        sparse_skips: []
    );

    roundtrip_tests!(
        pending_fact_row,
        full: PendingFactRow {
            fact_id: 1,
            subject: "Alice".to_string(),
            predicate: "ssn".to_string(),
            object: Some("123-45-6789".to_string()),
            created_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse: PendingFactRow {
            fact_id: 1,
            subject: "Alice".to_string(),
            predicate: "ssn".to_string(),
            object: None,
            created_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse_skips: ["object"]
    );

    #[test]
    fn pending_list_response_roundtrip() {
        let resp = PendingListResponse {
            total: 1,
            facts: vec![PendingFactRow {
                fact_id: 1,
                subject: "Alice".to_string(),
                predicate: "ssn".to_string(),
                object: None,
                created_at: "2020-01-01T00:00:00Z".to_string(),
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn confirm_fact_response_roundtrip() {
        let resp = ConfirmFactResponse {
            fact: sample_fact_row(),
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn reject_fact_request_defaults_and_roundtrip() {
        let parsed: RejectFactRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.reason, None);
        let req = RejectFactRequest {
            reason: Some("entered in error".to_string()),
        };
        assert_eq!(roundtrip(&req), req);
        let sparse = serde_json::to_string(&RejectFactRequest::default()).unwrap();
        assert!(!sparse.contains("reason"));
    }

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
}
