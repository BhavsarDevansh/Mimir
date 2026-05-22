use serde::{Deserialize, Serialize};

/// Permission level for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolPermission {
    /// Run immediately without user confirmation.
    Auto,
    /// Requires user approval (Phase 1: returns permission-denied error).
    #[default]
    Ask,
    /// Tool is disabled and cannot be invoked.
    Disabled,
}

impl ToolPermission {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolPermission::Auto => "auto",
            ToolPermission::Ask => "ask",
            ToolPermission::Disabled => "disabled",
        }
    }
}

impl std::str::FromStr for ToolPermission {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(ToolPermission::Auto),
            "ask" => Ok(ToolPermission::Ask),
            "disabled" => Ok(ToolPermission::Disabled),
            _ => Err(format!("invalid permission: {s}")),
        }
    }
}

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
