mod builtins;
mod cli;
mod config;
mod error;
mod output;
mod permission;
mod registry;

pub use builtins::{EchoTool, GetCurrentTimeTool, GetWeatherTool, SearchConversationHistoryTool};
pub use cli::{CliTool, CliToolConfig};
pub use config::ToolsConfig;
pub use error::ToolError;
pub use output::ToolOutput;
pub use output::output_to_llm_text;
pub use permission::ToolPermission;
pub use registry::{ToolEntry, ToolMetadata, ToolRegistry, ToolSource};

use async_trait::async_trait;
use serde_json::Value;

/// Convert a snake_case identifier to Title Case for display.
///
/// Examples: `get_current_time` → `Get Current Time`, `echo` → `Echo`.
pub fn snake_to_title_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Object-safe trait for all tools (native Rust and CLI wrappers).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name.
    fn name(&self) -> &str;

    /// Human-readable display name (defaults to Title Case conversion of `name()`).
    fn display_name(&self) -> &str {
        // Default is computed from name(); tools that want a custom label
        // should override this method. Because the trait is object-safe,
        // we return a static default here — the registry stores the
        // computed display_name in ToolMetadata at registration time.
        "Unnamed Tool"
    }

    /// Human-readable description for the LLM.
    fn description(&self) -> &str;

    /// JSON Schema object for the tool's parameters.
    /// Must return an object with `type: "object"`, `properties`, `required`, etc.
    fn parameters_schema(&self) -> Value;

    /// Default permission level for this tool.
    fn permission(&self) -> ToolPermission;

    /// Execute the tool with the given JSON arguments.
    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError>;

    /// Whether this tool mutates persistent state (e.g. writes facts to the
    /// knowledge graph). Incognito turns suppress write-capable tools so that
    /// no persistence occurs, honouring the incognito contract (issue #155).
    /// Defaults to `false` (read-only).
    fn is_write_tool(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_to_title_case_basic() {
        assert_eq!(snake_to_title_case("get_current_time"), "Get Current Time");
    }

    #[test]
    fn test_snake_to_title_case_single_word() {
        assert_eq!(snake_to_title_case("echo"), "Echo");
    }

    #[test]
    fn test_snake_to_title_case_memory() {
        assert_eq!(snake_to_title_case("memory"), "Memory");
    }

    #[test]
    fn test_snake_to_title_case_empty() {
        assert_eq!(snake_to_title_case(""), "");
    }

    #[test]
    fn test_snake_to_title_case_already_capital() {
        assert_eq!(snake_to_title_case("HTTP_client"), "HTTP Client");
    }

    #[test]
    fn test_snake_to_title_case_consecutive_underscores() {
        assert_eq!(snake_to_title_case("a__b"), "A  B");
    }

    #[test]
    fn test_display_name_default() {
        // The default display_name returns "Unnamed Tool" since it can't
        // call self.name() in a const-like context. The registry computes
        // the actual display name at registration time.
        // This test just verifies the trait method exists.
        struct DummyTool;
        #[async_trait::async_trait]
        impl Tool for DummyTool {
            fn name(&self) -> &str {
                "dummy"
            }
            fn description(&self) -> &str {
                "test"
            }
            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            fn permission(&self) -> ToolPermission {
                ToolPermission::Auto
            }
            async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput::default())
            }
        }
        let tool = DummyTool;
        assert_eq!(tool.display_name(), "Unnamed Tool");
    }
}
