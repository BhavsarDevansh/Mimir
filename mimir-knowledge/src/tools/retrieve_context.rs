//! `retrieve_context` — LLM-callable tool that spawns a RetrievalAgent.

use std::sync::Arc;

use async_trait::async_trait;
use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission, ToolProgress};
use serde::Deserialize;
use serde_json::Value;
use tracing::info;

use crate::KnowledgeGraph;
use crate::retrieval::RetrievalAgent;

/// Input schema for `retrieve_context`.
#[derive(Debug, Deserialize)]
struct RetrieveContextInput {
    /// Specific research task.
    task: String,
}

/// `retrieve_context` tool — the main agent uses this to launch a dedicated
/// RetrievalAgent that investigates the knowledge graph and conversation history.
pub struct RetrieveContextTool {
    kg: Arc<KnowledgeGraph>,
    context_manager: Arc<mimir_core::context::ContextManager>,
    progress: Option<tokio::sync::mpsc::Sender<ToolProgress>>,
}

impl RetrieveContextTool {
    /// Tool name used in the OpenAI-compatible function schema and registry.
    pub const NAME: &str = "retrieve_context";

    pub fn new(
        kg: Arc<KnowledgeGraph>,
        context_manager: Arc<mimir_core::context::ContextManager>,
    ) -> Self {
        Self {
            kg,
            context_manager,
            progress: None,
        }
    }

    /// Attach a progress channel so the retrieval agent's sub-tool calls can
    /// be streamed to the caller (issue #487).
    pub fn with_progress(mut self, progress: tokio::sync::mpsc::Sender<ToolProgress>) -> Self {
        self.progress = Some(progress);
        self
    }
}

#[async_trait]
impl Tool for RetrieveContextTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn display_name(&self) -> &str {
        "Retrieve Context"
    }

    fn description(&self) -> &str {
        "Launch a dedicated research agent to investigate the knowledge graph and conversation history. \
        Provide a specific task (e.g. 'Find Mary\'s food preferences and any allergies'). \
        The agent will query entities, traverse relationships, search past conversations, \
        and return a structured summary of findings. Call this whenever you need factual \
        or historical context before answering."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Specific research task. Describe the entity(ies) and what information you need."
                }
            },
            "required": ["task"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let input: RetrieveContextInput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments(Self::NAME, format!("invalid JSON args: {}", e))
        })?;

        let task = input.task.trim();
        if task.is_empty() {
            return Err(ToolError::invalid_arguments(
                Self::NAME,
                "task must be non-empty",
            ));
        }

        info!(task_len = task.len(), "spawning retrieval agent");

        let agent = RetrievalAgent::new(Arc::clone(&self.kg), Arc::clone(&self.context_manager));
        let mut agent = agent;
        if let Some(tx) = &self.progress {
            agent = agent.with_progress(tx.clone());
        }

        let context = agent
            .retrieve(task)
            .await
            .map_err(|e| ToolError::execution_failed(Self::NAME, e.to_string()))?;

        let result_json = serde_json::to_value(&context)
            .map_err(|e| ToolError::execution_failed(Self::NAME, e.to_string()))?;

        Ok(ToolOutput {
            result: Some(result_json),
            stdout: Some(context.summary()),
            ..Default::default()
        })
    }
}
