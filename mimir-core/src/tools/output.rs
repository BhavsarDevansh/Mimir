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
    /// Render a human-readable display string for the CLI.
    ///
    /// Extracts the primary value without `result:` prefix or JSON quotes.
    /// Falls back to error, stdout, or "(no output)" as needed.
    pub fn to_display_text(&self) -> String {
        if let Some(ref err) = self.error {
            return format!("error: {err}");
        }
        if let Some(ref val) = self.result {
            // Strip JSON quotes from string values for cleaner display.
            match val {
                serde_json::Value::String(s) => return s.clone(),
                other => return other.to_string(),
            }
        }
        if let Some(ref stdout) = self.stdout {
            let trimmed = stdout.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        String::from("(no output)")
    }

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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_text_prefers_error_over_result() {
        let out = ToolOutput {
            result: Some(serde_json::json!("ok")),
            error: Some("boom".to_string()),
            ..Default::default()
        };
        assert_eq!(out.to_display_text(), "error: boom");
    }

    #[test]
    fn display_text_string_result_unquotes() {
        let out = ToolOutput {
            result: Some(serde_json::json!("hello")),
            ..Default::default()
        };
        assert_eq!(out.to_display_text(), "hello");
    }

    #[test]
    fn display_text_non_string_result_jsonified() {
        let out = ToolOutput {
            result: Some(serde_json::json!({"a": 1})),
            ..Default::default()
        };
        assert_eq!(out.to_display_text(), r#"{"a":1}"#);
    }

    #[test]
    fn display_text_falls_back_to_trimmed_stdout() {
        let out = ToolOutput {
            stdout: Some("  line\n".to_string()),
            ..Default::default()
        };
        assert_eq!(out.to_display_text(), "line");
    }

    #[test]
    fn display_text_empty_stdout_falls_through_to_no_output() {
        let out = ToolOutput {
            stdout: Some("   ".to_string()),
            ..Default::default()
        };
        assert_eq!(out.to_display_text(), "(no output)");
    }

    #[test]
    fn display_text_no_fields_returns_placeholder() {
        let out = ToolOutput::default();
        assert_eq!(out.to_display_text(), "(no output)");
    }

    #[test]
    fn llm_text_joins_all_present_parts() {
        let out = ToolOutput {
            result: Some(serde_json::json!(42)),
            error: Some("e".to_string()),
            stdout: Some("out\n".to_string()),
            stderr: Some("err\n".to_string()),
            exit_code: Some(0),
        };
        let text = out.to_llm_text();
        assert!(text.contains("result: 42"));
        assert!(text.contains("error: e"));
        assert!(text.contains("stdout: out"));
        assert!(text.contains("stderr: err"));
        assert!(text.contains("exit_code: 0"));
        assert_eq!(text.lines().count(), 5);
    }

    #[test]
    fn llm_text_omits_empty_stdout_and_stderr() {
        let out = ToolOutput {
            stdout: Some("   ".to_string()),
            stderr: Some("\n".to_string()),
            ..Default::default()
        };
        assert_eq!(out.to_llm_text(), "(no output)");
    }

    #[test]
    fn llm_text_skips_serializing_none_fields_roundtrip() {
        let out = ToolOutput {
            result: Some(serde_json::json!("x")),
            ..Default::default()
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("result"));
        assert!(!json.contains("error"));
        assert!(!json.contains("stdout"));
        assert!(!json.contains("stderr"));
        assert!(!json.contains("exit_code"));
        let back: ToolOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(back, out);
    }

    #[test]
    fn output_to_llm_text_helper_directly() {
        let text = output_to_llm_text(
            Some(&serde_json::json!("hi")),
            None,
            None,
            None,
            None,
        );
        assert_eq!(text, "result: \"hi\"");
    }
}
