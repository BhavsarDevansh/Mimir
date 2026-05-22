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
        output_to_llm_text(
            self.result.as_ref(),
            self.error.as_ref(),
            self.stdout.as_ref(),
            self.stderr.as_ref(),
            self.exit_code,
        )
    }
}

/// Shared helper to render structured output fields as plaintext for LLM consumption.
pub fn output_to_llm_text(
    result: Option<&serde_json::Value>,
    error: Option<&String>,
    stdout: Option<&String>,
    stderr: Option<&String>,
    exit_code: Option<i32>,
) -> String {
    let mut parts = Vec::new();

    if let Some(result) = result {
        parts.push(format!("result: {result}"));
    }
    if let Some(error) = error {
        parts.push(format!("error: {error}"));
    }
    if let Some(stdout) = stdout {
        let trimmed = stdout.trim();
        if !trimmed.is_empty() {
            parts.push(format!("stdout: {trimmed}"));
        }
    }
    if let Some(stderr) = stderr {
        let trimmed = stderr.trim();
        if !trimmed.is_empty() {
            parts.push(format!("stderr: {trimmed}"));
        }
    }
    if let Some(code) = exit_code {
        parts.push(format!("exit_code: {code}"));
    }

    if parts.is_empty() {
        String::from("(no output)")
    } else {
        parts.join("\n")
    }
}
