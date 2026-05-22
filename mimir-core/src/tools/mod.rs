mod builtins;
mod cli;
mod config;
mod error;
mod output;
mod permission;
mod registry;

pub use builtins::{EchoTool, GetCurrentTimeTool};
pub use cli::{CliTool, CliToolConfig};
pub use config::ToolsConfig;
pub use error::ToolError;
pub use output::ToolOutput;
pub use output::output_to_llm_text;
pub use permission::ToolPermission;
pub use registry::{ToolEntry, ToolMetadata, ToolRegistry, ToolSource};

use async_trait::async_trait;
use serde_json::Value;

/// Object-safe trait for all tools (native Rust and CLI wrappers).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name.
    fn name(&self) -> &str;

    /// Human-readable description for the LLM.
    fn description(&self) -> &str;

    /// JSON Schema object for the tool's parameters.
    /// Must return an object with `type: "object"`, `properties`, `required`, etc.
    fn parameters_schema(&self) -> Value;

    /// Default permission level for this tool.
    fn permission(&self) -> ToolPermission;

    /// Execute the tool with the given JSON arguments.
    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError>;
}
