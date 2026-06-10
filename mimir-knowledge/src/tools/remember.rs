use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission};

use crate::KnowledgeGraph;
use crate::extract::{RememberOutput, process_remember_output};

/// Tool: remember
///
/// Accepts structured facts from the LLM and persists them to the knowledge graph.
/// Used when the LLM detects a fact worth saving during conversation.
pub struct RememberTool {
    kg: Arc<KnowledgeGraph>,
}

impl RememberTool {
    pub fn new(kg: Arc<KnowledgeGraph>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "Extract and persist structured facts to the knowledge graph.          Use this tool whenever the user shares information about themselves,          their preferences, their life, or anything you should remember for future          conversations. Each fact is a subject-relationship_type-object triple with          classification, temporal bounds, and sensitivity flags."
    }

    fn parameters_schema(&self) -> Value {
        crate::extract::remember_tool_params_schema()
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let output: RememberOutput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments("remember", format!("invalid JSON args: {}", e))
        })?;

        match process_remember_output(&self.kg, output).await {
            Ok(outcome) => {
                let mut parts = Vec::new();
                if !outcome.inserted.is_empty() {
                    parts.push(format!("{} fact(s) inserted.", outcome.inserted.len()));
                }
                if !outcome.pending_confirmation.is_empty() {
                    parts.push(format!(
                        "{} sensitive fact(s) awaiting user confirmation.",
                        outcome.pending_confirmation.len()
                    ));
                }
                if !outcome.corroborated.is_empty() {
                    parts.push(format!(
                        "{} fact(s) matched existing records.",
                        outcome.corroborated.len()
                    ));
                }
                if !outcome.errors.is_empty() {
                    parts.push(format!(
                        "{} error(s) during processing.",
                        outcome.errors.len()
                    ));
                }
                if parts.is_empty() {
                    parts.push("No facts extracted or persisted.".to_string());
                }
                Ok(ToolOutput {
                    result: Some(Value::String(parts.join(" "))),
                    ..Default::default()
                })
            }
            Err(e) => Err(ToolError::execution_failed(
                "remember",
                format!("processing failed: {}", e),
            )),
        }
    }
}
