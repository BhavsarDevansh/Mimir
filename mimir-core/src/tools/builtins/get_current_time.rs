use super::super::{Tool, ToolError, ToolOutput, ToolPermission};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

/// Returns the current time in RFC 3339 format.
pub struct GetCurrentTimeTool;

#[async_trait]
impl Tool for GetCurrentTimeTool {
    fn name(&self) -> &str {
        "get_current_time"
    }

    fn description(&self) -> &str {
        "Returns the current date and time in RFC 3339 format."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false,
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let now = Utc::now().to_rfc3339();
        Ok(ToolOutput {
            result: Some(Value::String(now)),
            ..Default::default()
        })
    }
}
