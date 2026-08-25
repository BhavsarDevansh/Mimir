//! Shared LLM tool-call plumbing for the semantic-dedup passes.
//!
//! The fact-level `semantic_dedup` pass and the entity-level
//! `enqueue_semantic_dedup` path both send a JSON payload to the LLM under a
//! strict single-tool schema and parse the assistant's tool call back into a
//! typed response. Owning that contract here keeps the two call sites in
//! sync (issue #282 review): the system prompt names the tool, the user
//! message carries the serialized payload, and the assistant reply must be
//! exactly one call to the expected tool, parsed via
//! [`mimir_core::llm::parse_tool_output`].

use std::sync::Arc;

use mimir_core::llm::types::Message;
use mimir_core::llm::{LlmBackend, parse_tool_output};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::KnowledgeError;

/// Send `payload` to the LLM under a strict single-tool schema and parse the
/// assistant's tool call as `T`.
///
/// `label` names the caller in error messages (e.g. `"semantic dedup"`).
pub(crate) async fn call_dedup_tool<T: DeserializeOwned>(
    llm: &Arc<dyn LlmBackend>,
    tool: serde_json::Value,
    payload: &impl Serialize,
    label: &str,
) -> Result<T, KnowledgeError> {
    let tool_name = tool
        .pointer("/function/name")
        .and_then(|n| n.as_str())
        .unwrap_or(label)
        .to_string();
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: format!("Use the {tool_name} tool to return your evaluation."),
            tool_calls: None,
            tool_call_id: None,
        },
        Message {
            role: "user".to_string(),
            content: serde_json::to_string(payload)
                .map_err(|e| KnowledgeError::Validation(format!("{label} JSON error: {e}")))?,
            tool_calls: None,
            tool_call_id: None,
        },
    ];
    let (assistant_msg, _) = llm
        .chat_message(messages, Some(vec![tool]))
        .await
        .map_err(|e| KnowledgeError::Validation(format!("{label} LLM error: {e}")))?;
    parse_tool_output::<T>(assistant_msg, Some(&tool_name))
        .map_err(|e| KnowledgeError::Validation(format!("{label} tool-call error: {e}")))
}
