use crate::llm::types::Message;
use crate::skills::{Skill, SkillContext, SkillError, SkillInput, SkillOutput};
use crate::tools::ToolPermission;
use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

/// Generates a Test-Driven Development plan for a given task.
///
/// Returns a structured red-green-refactor workflow as JSON.
pub struct TestDrivenDevelopmentSkill;

#[async_trait]
impl Skill for TestDrivenDevelopmentSkill {
    fn name(&self) -> &str {
        "test_driven_development"
    }

    fn description(&self) -> &str {
        "Generates a Test-Driven Development (TDD) plan for a given programming task. Returns a structured red-green-refactor workflow."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The programming task to generate a TDD plan for."
                }
            },
            "required": ["task"],
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
        let task = input
            .args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SkillError::invalid_arguments(self.name(), "missing 'task' argument"))?;

        debug!(skill = %self.name(), task = %task, "executing TDD skill");

        let system_prompt = "You are a TDD planning engine.\n\n\
            For the given task, produce a structured TDD plan following this exact JSON schema:\n\n\
            {\n  \"plan\": [\n    {\n      \"phase\": \"red\",\n      \"description\": \"...\",\n      \"tests\": [\"...\"]\n    },\n    {\n      \"phase\": \"green\",\n      \"description\": \"...\",\n      \"implementation_steps\": [\"...\"]\n    },\n    {\n      \"phase\": \"refactor\",\n      \"description\": \"...\",\n      \"refactor_targets\": [\"...\"]\n    }\n  ]\n}\n\n\
            Return ONLY valid JSON.";

        let messages = vec![
            Message::system(system_prompt),
            Message::user(format!("Task: {task}\n\nGenerate the TDD plan.")),
        ];

        match ctx.llm_client.chat(messages).await {
            Ok((content, _usage)) => {
                // Try to parse as JSON to validate it.
                match serde_json::from_str::<Value>(&content) {
                    Ok(json) => Ok(SkillOutput {
                        result: Some(json),
                        ..Default::default()
                    }),
                    Err(_) => {
                        // If the LLM didn't return pure JSON, wrap the raw text.
                        Ok(SkillOutput {
                            result: Some(Value::String(content)),
                            ..Default::default()
                        })
                    }
                }
            }
            Err(e) => Err(SkillError::execution_failed(
                self.name(),
                format!("LLM error: {e}"),
            )),
        }
    }
}
