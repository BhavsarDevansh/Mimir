use super::super::{Tool, ToolError, ToolOutput, ToolPermission};
use async_trait::async_trait;
use serde_json::Value;

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

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
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
