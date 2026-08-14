//! Shared parsing of LLM tool-call output.
//!
//! Both the conversational `remember` extraction path
//! (`mimir-knowledge::extract::parse`) and the Email connector's LLM prose
//! extraction (`mimir-connectors::email::llm::parse`) turn an assistant
//! [`Message`] into a typed tool output. The two parsers used to duplicate
//! the same three-step dance — take the first `tool_calls` entry and parse
//! its `function.arguments`, else strip a ```fence``` from `content` and
//! parse the JSON, else error (issue #259). This module owns that dance once;
//! callers map [`ToolOutputParseError`] onto their own error types.

use serde::de::DeserializeOwned;

use crate::llm::types::{Message, ToolCall};

/// Error returned when an assistant message cannot be parsed as a typed tool
/// output.
#[derive(Debug, thiserror::Error)]
pub enum ToolOutputParseError {
    /// The assistant emitted an empty `tool_calls` list.
    #[error("LLM tool call list was empty.")]
    EmptyToolCalls,
    /// The assistant emitted more than one tool call, but the caller expects
    /// exactly one named call.
    #[error("LLM returned {count} tool calls; expected exactly one `{expected}` call.")]
    TooManyToolCalls { count: usize, expected: String },
    /// The assistant called a different tool than the caller expects.
    #[error("LLM returned tool call `{actual}`; expected `{expected}`.")]
    UnexpectedToolName { actual: String, expected: String },
    /// A tool call's `arguments` were not valid JSON for `T`.
    #[error("Failed to parse tool arguments: {0}")]
    InvalidArguments(#[from] serde_json::Error),
    /// The assistant emitted no tool call and no content to parse.
    #[error("LLM did not emit a tool call.")]
    NoToolCall,
    /// The assistant emitted no tool call and its content was not valid JSON
    /// for `T`. `text` carries the fence-stripped content so callers with a
    /// second wire shape (e.g. the conversational bare-array fallback) can
    /// retry the parse.
    #[error("LLM did not emit a tool call and response could not be parsed as JSON: {head}")]
    InvalidJson { head: String, text: String },
}

/// Parse an assistant message into a typed tool output.
///
/// Handles the two shapes LLM backends emit:
///
/// 1. **Tool call** — the first `tool_calls` entry's `function.arguments`
///    are parsed as `T`.
/// 2. **Content fallback** — a ```fence``` is stripped from `content` (if
///    present) and the remainder is parsed as `T`.
///
/// When `expected_tool_name` is `Some`, the tool-call path additionally
/// rejects a multi-call completion and a call to any other tool, so arguments
/// from a different function can never be deserialized as `T`. When `None`,
/// the first call is used without name or count checks (the conversational
/// path's legacy behaviour).
pub fn parse_tool_output<T: DeserializeOwned>(
    message: Message,
    expected_tool_name: Option<&str>,
) -> Result<T, ToolOutputParseError> {
    if let Some(tool_calls) = message.tool_calls {
        if let Some(expected) = expected_tool_name {
            if tool_calls.len() > 1 {
                return Err(ToolOutputParseError::TooManyToolCalls {
                    count: tool_calls.len(),
                    expected: expected.to_string(),
                });
            }
            let first = tool_calls
                .first()
                .ok_or(ToolOutputParseError::EmptyToolCalls)?;
            if first.function.name != expected {
                return Err(ToolOutputParseError::UnexpectedToolName {
                    actual: first.function.name.clone(),
                    expected: expected.to_string(),
                });
            }
        }
        return parse_first_tool_call::<T>(tool_calls);
    }

    let text = message.content.trim();
    if text.is_empty() {
        return Err(ToolOutputParseError::NoToolCall);
    }
    let json_text = strip_code_fence(text);
    serde_json::from_str::<T>(&json_text).map_err(|_| ToolOutputParseError::InvalidJson {
        head: json_text.chars().take(200).collect(),
        text: json_text,
    })
}

/// Parse the first tool call's `arguments` as `T`.
fn parse_first_tool_call<T: DeserializeOwned>(
    tool_calls: Vec<ToolCall>,
) -> Result<T, ToolOutputParseError> {
    let first = tool_calls
        .into_iter()
        .next()
        .ok_or(ToolOutputParseError::EmptyToolCalls)?;
    serde_json::from_str(&first.function.arguments).map_err(ToolOutputParseError::InvalidArguments)
}

/// Return the JSON text from an assistant reply, stripping a ```fence``` if
/// the model wrapped its output. Returns an owned `String`: fence stripping
/// re-joins the inner lines, and callers keep the text after the `Message`
/// is consumed to retry parsing with a second wire shape.
fn strip_code_fence(text: &str) -> String {
    let text = text.trim();
    if !text.starts_with("```") {
        return text.to_string();
    }
    text.lines()
        .skip_while(|l| l.starts_with("```"))
        .take_while(|l| !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::FunctionCall;

    fn message_with_tool_calls(tool_calls: Vec<ToolCall>) -> Message {
        Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    fn tool_call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            index: 0,
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn content_message(content: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn parses_tool_call_arguments() {
        let msg = message_with_tool_calls(vec![tool_call("remember", r#"{"facts": []}"#)]);
        let out: serde_json::Value = parse_tool_output(msg, None).expect("tool call parses");
        assert_eq!(out["facts"], serde_json::json!([]));
    }

    #[test]
    fn without_expected_name_takes_first_call() {
        let msg = message_with_tool_calls(vec![
            tool_call("remember", r#"{"facts": [1]}"#),
            tool_call("other", r#"{"facts": [2]}"#),
        ]);
        let out: serde_json::Value = parse_tool_output(msg, None).expect("first call parses");
        assert_eq!(out["facts"], serde_json::json!([1]));
    }

    #[test]
    fn expected_name_accepts_matching_call() {
        let msg =
            message_with_tool_calls(vec![tool_call("extract_email_facts", r#"{"facts": []}"#)]);
        let out: serde_json::Value =
            parse_tool_output(msg, Some("extract_email_facts")).expect("matching call parses");
        assert_eq!(out["facts"], serde_json::json!([]));
    }

    #[test]
    fn expected_name_rejects_wrong_tool() {
        let msg = message_with_tool_calls(vec![tool_call("summarise_email", r#"{"facts": []}"#)]);
        let err = parse_tool_output::<serde_json::Value>(msg, Some("extract_email_facts"))
            .expect_err("wrong tool name rejected");
        assert!(matches!(
            err,
            ToolOutputParseError::UnexpectedToolName { actual, expected }
                if actual == "summarise_email" && expected == "extract_email_facts"
        ));
    }

    #[test]
    fn expected_name_rejects_multiple_calls() {
        let msg = message_with_tool_calls(vec![
            tool_call("extract_email_facts", r#"{"facts": []}"#),
            tool_call("other_tool", "{}"),
        ]);
        let err = parse_tool_output::<serde_json::Value>(msg, Some("extract_email_facts"))
            .expect_err("multi-call completion rejected");
        assert!(matches!(
            err,
            ToolOutputParseError::TooManyToolCalls { count: 2, .. }
        ));
    }

    #[test]
    fn expected_name_rejects_empty_call_list() {
        let msg = message_with_tool_calls(Vec::new());
        let err = parse_tool_output::<serde_json::Value>(msg, Some("extract_email_facts"))
            .expect_err("empty list rejected");
        assert!(matches!(err, ToolOutputParseError::EmptyToolCalls));
    }

    #[test]
    fn rejects_empty_tool_call_list() {
        let msg = message_with_tool_calls(Vec::new());
        let err =
            parse_tool_output::<serde_json::Value>(msg, None).expect_err("empty list rejected");
        assert!(matches!(err, ToolOutputParseError::EmptyToolCalls));
    }

    #[test]
    fn rejects_invalid_tool_arguments() {
        let msg = message_with_tool_calls(vec![tool_call("remember", "not json")]);
        let err = parse_tool_output::<serde_json::Value>(msg, None).expect_err("bad args rejected");
        assert!(matches!(err, ToolOutputParseError::InvalidArguments(_)));
    }

    #[test]
    fn parses_plain_content_fallback() {
        let msg = content_message(r#"{"facts": [1]}"#);
        let out: serde_json::Value = parse_tool_output(msg, None).expect("content parses");
        assert_eq!(out["facts"], serde_json::json!([1]));
    }

    #[test]
    fn parses_fenced_content_fallback() {
        let msg = content_message("```json\n{\"facts\": [1]}\n```");
        let out: serde_json::Value = parse_tool_output(msg, None).expect("fenced content parses");
        assert_eq!(out["facts"], serde_json::json!([1]));
    }

    #[test]
    fn rejects_empty_content() {
        let msg = content_message("   ");
        let err =
            parse_tool_output::<serde_json::Value>(msg, None).expect_err("empty content rejected");
        assert!(matches!(err, ToolOutputParseError::NoToolCall));
    }

    #[test]
    fn rejects_invalid_content_json_and_carries_text() {
        let msg = content_message("```json\nnot json at all\n```");
        let err =
            parse_tool_output::<serde_json::Value>(msg, None).expect_err("bad content rejected");
        match err {
            ToolOutputParseError::InvalidJson { text, .. } => {
                assert_eq!(
                    text, "not json at all",
                    "fence-stripped text for fallback retry"
                );
            }
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }
}
