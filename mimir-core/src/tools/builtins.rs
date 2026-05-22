use super::{Tool, ToolError, ToolOutput};
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

    fn permission(&self) -> super::ToolPermission {
        super::ToolPermission::Auto
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let now = Utc::now().to_rfc3339();
        Ok(ToolOutput {
            result: Some(Value::String(now)),
            ..Default::default()
        })
    }
}

/// Echoes back the provided message.
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes back the provided message. Useful for testing and verification."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to echo back."
                }
            },
            "required": ["message"],
            "additionalProperties": false,
        })
    }

    fn permission(&self) -> super::ToolPermission {
        super::ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_arguments("echo", "missing 'message' argument"))?;

        Ok(ToolOutput {
            result: Some(Value::String(message.to_string())),
            ..Default::default()
        })
    }
}
