mod builtins;
pub mod error;
pub mod generated;
pub mod markdown;
pub mod metrics;
pub mod permissions_config;
pub mod registry;

pub use builtins::{ResearchSynthesisSkill, TestDrivenDevelopmentSkill};
pub use error::SkillError;
pub use generated::{GeneratedSkillCandidate, SessionSummary, should_generate_skill};
pub use markdown::{MarkdownSkill, SkillDefinition, parse_skill_file};
pub use metrics::SkillMetricsDb;
pub use permissions_config::SkillsPermissionsConfig;
pub use registry::{SkillEntry, SkillMetadata, SkillRegistry};

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::context::ContextManager;
use crate::llm::client::LlmClient;
use crate::tools::{ToolPermission, ToolRegistry};

/// Source of a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    Builtin,
    User,
    Generated,
}

impl SkillSource {
    /// Return the lowercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            SkillSource::Builtin => "builtin",
            SkillSource::User => "user",
            SkillSource::Generated => "generated",
        }
    }
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Input passed to a skill when invoked.
#[derive(Debug, Clone)]
pub struct SkillInput {
    /// The parsed JSON arguments from the LLM function call.
    pub args: Value,
}

/// Structured output from a skill execution.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct SkillOutput {
    /// Primary result value (JSON-serializable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message if the skill failed internally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Captured stdout (for subprocess-based skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// Captured stderr (for subprocess-based skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Exit code (for subprocess-based skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl SkillOutput {
    /// Render a compact plaintext representation for the LLM context.
    pub fn to_llm_text(&self) -> String {
        crate::tools::output_to_llm_text(
            self.result.as_ref(),
            self.error.as_ref(),
            self.stdout.as_ref(),
            self.stderr.as_ref(),
            self.exit_code,
        )
    }
}

/// Context shared with every skill, giving it access to the same
/// tools, LLM client, and conversation state as the core agent.
pub struct SkillContext {
    /// Registry of available tools the skill may invoke.
    pub tool_registry: Arc<ToolRegistry>,
    /// HTTP client for LLM API calls.
    pub llm_client: Arc<LlmClient>,
    /// Manager for persistent conversation history.
    pub context_manager: Arc<ContextManager>,
    /// The active session ID, if running inside a conversation.
    pub session_id: Option<String>,
}

impl SkillContext {
    /// Create a new skill context.
    pub fn new(
        tool_registry: Arc<ToolRegistry>,
        llm_client: Arc<LlmClient>,
        context_manager: Arc<ContextManager>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            tool_registry,
            llm_client,
            context_manager,
            session_id,
        }
    }
}

impl fmt::Debug for SkillContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkillContext")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Object-safe trait for all skills (built-in Rust, user Markdown, or generated).
#[async_trait]
pub trait Skill: Send + Sync {
    /// Unique skill name.
    fn name(&self) -> &str;

    /// Human-readable description for the LLM.
    fn description(&self) -> &str;

    /// JSON Schema object for the skill's parameters.
    fn parameters_schema(&self) -> Value;

    /// Default permission level for this skill.
    fn permission(&self) -> ToolPermission;

    /// Execute the skill with the given context and input.
    async fn execute(
        &self,
        ctx: SkillContext,
        input: SkillInput,
    ) -> Result<SkillOutput, SkillError>;
}
