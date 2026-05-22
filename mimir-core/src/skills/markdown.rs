use super::{Skill, SkillContext, SkillError, SkillInput, SkillOutput};
use crate::tools::ToolPermission;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

/// Maximum size of a skill file in bytes (1 MiB).
const MAX_SKILL_FILE_SIZE: usize = 1_048_576;

/// Parsed YAML frontmatter from a skill Markdown file.
/// YAML frontmatter extracted from a skill Markdown file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillFrontmatter {
    /// Unique skill identifier (snake_case recommended).
    pub name: String,
    /// Semantic version of the skill.
    #[serde(default = "default_version")]
    pub version: String,
    /// Optional author attribution.
    #[serde(default)]
    pub author: Option<String>,
    /// Human-readable description for the LLM.
    #[serde(default)]
    pub description: Option<String>,
    /// Categorical tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional JSON Schema for parameters. If absent, defaults to a single `query` string.
    #[serde(default)]
    pub parameters: Option<Value>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// A skill definition parsed from a Markdown file.
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub source_path: Option<std::path::PathBuf>,
}

/// Parse a Markdown skill file into frontmatter + body.
///
/// Expected format:
/// ```markdown
/// ---
/// name: weekly-summary
/// version: 1.0.0
/// ---
///
/// # Weekly Summary Skill
/// ...
/// ```
/// Parse a Markdown skill file into frontmatter + body.
///
/// Expected format:
/// ```markdown
/// ---
/// name: weekly-summary
/// version: 1.0.0
/// ---
///
/// # Weekly Summary Skill
/// ...
/// ```
///
/// Rejects files larger than [`MAX_SKILL_FILE_SIZE`].
pub fn parse_skill_file(contents: &str) -> Result<SkillDefinition, SkillError> {
    if contents.len() > MAX_SKILL_FILE_SIZE {
        return Err(SkillError::parse_error(
            "unparsed",
            format!("skill file exceeds maximum size of {MAX_SKILL_FILE_SIZE} bytes"),
        ));
    }

    let trimmed = contents.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillError::parse_error(
            "unparsed",
            "skill file must start with YAML frontmatter delimited by '---'",
        ));
    }

    // Find the second --- on its own line.
    let after_first = &trimmed[3..];
    let mut end_idx = None;
    for (idx, line) in after_first.lines().enumerate() {
        if line.trim() == "---" {
            // Compute byte offset: sum of all previous line lengths + newline chars.
            let offset: usize = after_first.lines().take(idx).map(|l| l.len() + 1).sum();
            end_idx = Some(offset);
            break;
        }
    }
    let Some(end_idx) = end_idx else {
        return Err(SkillError::parse_error(
            "unparsed",
            "YAML frontmatter is not closed with a standalone '---' line",
        ));
    };

    let yaml_str = &after_first[..end_idx].trim();
    let body = after_first[end_idx + 3..].trim_start().to_string();

    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_str).map_err(|e| {
        SkillError::parse_error("unparsed", format!("invalid YAML frontmatter: {e}"))
    })?;

    if frontmatter.name.is_empty() {
        return Err(SkillError::parse_error(
            "unparsed",
            "skill frontmatter must include a non-empty 'name' field",
        ));
    }

    Ok(SkillDefinition {
        frontmatter,
        body,
        source_path: None,
    })
}

/// A skill backed by a Markdown file with YAML frontmatter.
///
/// When invoked, the body is sent to the LLM as a system prompt,
/// with the input arguments serialized into the user message.
pub struct MarkdownSkill {
    pub name: String,
    pub version: String,
    pub tags: Vec<String>,
    pub description: String,
    pub parameters: Value,
    pub body: String,
}

impl MarkdownSkill {
    pub fn from_definition(def: SkillDefinition) -> Result<Self, SkillError> {
        let name = def.frontmatter.name.clone();
        let version = def.frontmatter.version.clone();
        let tags = def.frontmatter.tags.clone();
        let description = def
            .frontmatter
            .description
            .clone()
            .unwrap_or_else(|| format!("User-defined skill: {}", def.frontmatter.name));

        let parameters = def.frontmatter.parameters.clone().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The user's request or input for this skill."
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            })
        });

        // Normalize parameters to a JSON Schema object if it isn't already.
        let parameters = if parameters.get("type").and_then(|v| v.as_str()) == Some("object") {
            parameters
        } else {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": parameters
                },
                "required": ["value"],
                "additionalProperties": false,
            })
        };

        Ok(Self {
            name,
            version,
            tags,
            description,
            parameters,
            body: def.body,
        })
    }
}

#[async_trait]
impl Skill for MarkdownSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(
        &self,
        ctx: SkillContext,
        input: SkillInput,
    ) -> Result<SkillOutput, SkillError> {
        debug!(skill = %self.name, "executing markdown skill");

        let system_prompt = format!(
            "You are executing the skill '{}'.\n\n{}",
            self.name, self.body
        );

        let user_content = format!(
            "Skill input arguments:\n{}\n\nPlease execute the skill steps and return the result.",
            serde_json::to_string_pretty(&input.args).unwrap_or_default()
        );

        let messages = vec![
            crate::llm::types::Message::system(system_prompt),
            crate::llm::types::Message::user(user_content),
        ];

        match ctx.llm_client.chat(messages).await {
            Ok((content, _usage)) => Ok(SkillOutput {
                result: Some(Value::String(content)),
                ..Default::default()
            }),
            Err(e) => {
                warn!(skill = %self.name, error = %e, "LLM call failed for markdown skill");
                Err(SkillError::execution_failed(
                    &self.name,
                    format!("LLM error: {e}"),
                ))
            }
        }
    }
}

/// Build SkillMetadata from a MarkdownSkill and its source path.
/// Build SkillMetadata from a MarkdownSkill and its source path.
///
/// Uses the frontmatter version and tags when available.
pub fn build_metadata(
    skill: &MarkdownSkill,
    version: &str,
    tags: &[String],
    source_path: Option<&std::path::Path>,
) -> super::registry::SkillMetadata {
    super::registry::SkillMetadata {
        name: skill.name().to_string(),
        description: skill.description().to_string(),
        source: super::SkillSource::User,
        permission: ToolPermission::Auto,
        version: version.to_string(),
        tags: tags.to_vec(),
        source_path: source_path.map(|p| p.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_skill_file() {
        let content = r#"---
name: weekly-summary
version: 1.0.0
description: Summarize my week.
tags: [productivity, summary]
---

# Weekly Summary

## Steps
1. Query calendar
2. Synthesize narrative
"#;
        let def = parse_skill_file(content).unwrap();
        assert_eq!(def.frontmatter.name, "weekly-summary");
        assert_eq!(def.frontmatter.version, "1.0.0");
        assert_eq!(
            def.frontmatter.description.as_deref(),
            Some("Summarize my week.")
        );
        assert_eq!(def.frontmatter.tags, vec!["productivity", "summary"]);
        assert!(def.body.contains("# Weekly Summary"));
    }

    #[test]
    fn parse_missing_frontmatter_fails() {
        let content = "# Just a markdown file\n\nNo frontmatter here.";
        let result = parse_skill_file(content);
        assert!(matches!(result, Err(SkillError::ParseError(_, _))));
    }

    #[test]
    fn parse_unclosed_frontmatter_fails() {
        let content = "---\nname: foo\n";
        let result = parse_skill_file(content);
        assert!(matches!(result, Err(SkillError::ParseError(_, _))));
    }

    #[test]
    fn parse_missing_name_fails() {
        let content = "---\nversion: 1.0.0\n---\n\n# Body";
        let result = parse_skill_file(content);
        assert!(matches!(result, Err(SkillError::ParseError(_, _))));
    }

    #[test]
    fn markdown_skill_default_parameters() {
        let def = SkillDefinition {
            frontmatter: SkillFrontmatter {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
                author: None,
                description: None,
                tags: vec![],
                parameters: None,
            },
            body: "Do something.".to_string(),
            source_path: None,
        };
        let skill = MarkdownSkill::from_definition(def).unwrap();
        let schema = skill.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].get("query").is_some());
    }

    #[test]
    fn markdown_skill_custom_parameters() {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "topic": { "type": "string" }
            },
            "required": ["topic"]
        });
        let def = SkillDefinition {
            frontmatter: SkillFrontmatter {
                name: "custom".to_string(),
                version: "1.0.0".to_string(),
                author: None,
                description: None,
                tags: vec![],
                parameters: Some(params.clone()),
            },
            body: "Do something.".to_string(),
            source_path: None,
        };
        let skill = MarkdownSkill::from_definition(def).unwrap();
        assert_eq!(skill.parameters_schema(), params);
    }
}
