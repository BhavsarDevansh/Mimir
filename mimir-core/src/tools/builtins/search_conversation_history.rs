use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::context::ContextManager;
use crate::tools::{Tool, ToolError, ToolOutput, ToolPermission};

/// Built-in tool that searches conversation history via FTS5.
pub struct SearchConversationHistoryTool {
    context_manager: Arc<ContextManager>,
}

impl SearchConversationHistoryTool {
    pub fn new(context_manager: Arc<ContextManager>) -> Self {
        Self { context_manager }
    }
}

#[async_trait]
impl Tool for SearchConversationHistoryTool {
    fn name(&self) -> &str {
        "search_conversation_history"
    }

    fn display_name(&self) -> &str {
        "Search Conversation History"
    }

    fn description(&self) -> &str {
        "Searches past conversation history and returns contextual snippets around matches."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to look for in conversation history."
                },
                "limit": {
                    "type": "integer",
                    "default": 5,
                    "maximum": 20,
                    "description": "Maximum number of results to return."
                },
                "session_id": {
                    "type": "integer",
                    "description": "Optional session ID to restrict the search to a single conversation."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let session_id = args.get("session_id").and_then(|v| v.as_i64());

        let results = self
            .context_manager
            .search_messages(query, limit, session_id)
            .await
            .map_err(|e| ToolError::execution_failed(self.name(), e.to_string()))?;

        let json = serde_json::to_value(&results)
            .map_err(|e| ToolError::execution_failed(self.name(), e.to_string()))?;

        Ok(ToolOutput {
            result: Some(json),
            ..Default::default()
        })
    }
}
