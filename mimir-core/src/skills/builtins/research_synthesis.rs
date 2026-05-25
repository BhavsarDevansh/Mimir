use crate::llm::types::Message;
use crate::skills::{Skill, SkillContext, SkillError, SkillInput, SkillOutput};
use crate::tools::ToolPermission;
use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

/// Synthesizes a research narrative from a topic using the LLM.
///
/// Demonstrates the Skill trait by invoking the LLM client with a
/// structured system prompt that guides the research workflow.
pub struct ResearchSynthesisSkill;

#[async_trait]
impl Skill for ResearchSynthesisSkill {
    fn name(&self) -> &str {
        "research_synthesis"
    }

    fn description(&self) -> &str {
        "Synthesizes a research narrative from a given topic. Uses web search reasoning, fact extraction, and causal chain building to produce a coherent narrative."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "description": "The research topic or question to synthesize."
                }
            },
            "required": ["topic"],
            "additionalProperties": false,
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(
        &self,
        ctx: SkillContext,
        input: SkillInput,
    ) -> Result<SkillOutput, SkillError> {
        let topic = input
            .args
            .get("topic")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                SkillError::invalid_arguments(self.name(), "missing 'topic' argument")
            })?;

        debug!(skill = %self.name(), topic = %topic, "executing research synthesis");

        // Attempt to call get_current_time for temporal grounding if available.
        let time_context = if let Ok(output) = ctx
            .tool_registry
            .execute("get_current_time", serde_json::json!({}))
            .await
        {
            output.to_llm_text()
        } else {
            String::new()
        };

        let system_prompt = format!(
            "You are a research synthesis engine.\n\n\
            Follow this workflow:\n\
            1. Break the topic into sub-questions.\n\
            2. Identify key facts and causal relationships.\n\
            3. Synthesize a coherent narrative.\n\n\
            Current time context: {time_context}",
        );

        let messages = vec![
            Message::system(system_prompt),
            Message::user(format!(
                "Topic: {topic}\n\nPlease synthesize a research narrative."
            )),
        ];

        match ctx.llm_client.chat(messages, None).await {
            Ok((content, _usage)) => Ok(SkillOutput {
                result: Some(Value::String(content)),
                ..Default::default()
            }),
            Err(e) => Err(SkillError::execution_failed(
                self.name(),
                format!("LLM error: {e}"),
            )),
        }
    }
}
