use serde::{Deserialize, Serialize};

/// Structured output from a tool execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolOutput {
    /// Primary result value (JSON-serializable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error message if the tool failed internally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Captured stdout (for CLI tools).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// Captured stderr (for CLI tools).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Exit code (for CLI tools).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl ToolOutput {
    /// Render a compact plaintext representation for the LLM context.
    pub fn to_llm_text(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref result) = self.result {
            parts.push(format!("result: {result}"));
        }
        if let Some(ref error) = self.error {
            parts.push(format!("error: {error}"));
        }
        if let Some(ref stdout) = self.stdout {
            let trimmed = stdout.trim();
            if !trimmed.is_empty() {
                parts.push(format!("stdout: {trimmed}"));
            }
        }
        if let Some(ref stderr) = self.stderr {
            let trimmed = stderr.trim();
            if !trimmed.is_empty() {
                parts.push(format!("stderr: {trimmed}"));
            }
        }
        if let Some(code) = self.exit_code {
            parts.push(format!("exit_code: {code}"));
        }

        if parts.is_empty() {
            String::from("(no output)")
        } else {
            parts.join("\n")
        }
    }
}
