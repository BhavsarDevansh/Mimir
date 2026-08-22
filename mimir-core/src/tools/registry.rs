use super::{CliTool, CliToolConfig, Tool, ToolError, ToolOutput, ToolPermission};
use crate::llm::LlmBackend;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// Per-request runtime dependencies for tool execution.
///
/// Tools registered with a factory receive this context so they can be
/// rebuilt with request-scoped dependencies — such as the request-resolved
/// LLM with model/temperature overrides — that the registry's pre-built
/// instances cannot carry (issue #441).
pub struct ToolContext {
    /// The request-resolved LLM backend.
    pub llm: Arc<dyn LlmBackend>,
    /// Whether write-capable tools may execute. Incognito turns pass `false`
    /// so the registry blocks write tools uniformly (issue #155).
    pub allow_write_tools: bool,
}

impl ToolContext {
    /// Create a new tool context.
    pub fn new(llm: Arc<dyn LlmBackend>, allow_write_tools: bool) -> Self {
        Self {
            llm,
            allow_write_tools,
        }
    }
}

/// Rebuilds a tool with per-request runtime dependencies from a [`ToolContext`].
pub type ToolFactory = Arc<dyn Fn(&ToolContext) -> Arc<dyn Tool> + Send + Sync>;

/// Source of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    Native,
    Cli,
}

/// Metadata for a registered tool.
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub source: ToolSource,
    pub permission: ToolPermission,
    /// Whether the tool mutates persistent state (issue #155).
    pub is_write_tool: bool,
}

/// Entry in the registry combining the tool implementation with metadata.
pub struct ToolEntry {
    pub tool: Arc<dyn Tool>,
    pub metadata: ToolMetadata,
    /// Original CLI config, if this is a CLI tool.
    pub cli_config: Option<CliToolConfig>,
    /// Optional factory that rebuilds the tool with per-request runtime
    /// dependencies (issue #441). When present, `execute` calls the factory
    /// with the request context instead of using the stored instance.
    pub factory: Option<ToolFactory>,
}

/// Dynamic registry for tool discovery, registration, and invocation.
pub struct ToolRegistry {
    entries: RwLock<HashMap<String, ToolEntry>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let entries = self.entries.read().unwrap();
        f.debug_struct("ToolRegistry")
            .field("tool_count", &entries.len())
            .field("tool_names", &entries.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with all built-in tools registered.
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        registry.register_builtins();
        registry
    }

    /// Register native built-in tools.
    pub fn register_builtins(&self) {
        // Built-in names are hardcoded and guaranteed unique; unwrap is safe.
        let _ = self.register_native(Arc::new(super::GetCurrentTimeTool));
        let _ = self.register_native(Arc::new(super::EchoTool));
        let _ = self.register_native(Arc::new(super::GetWeatherTool::new()));
    }

    /// Register a native tool with its default permission.
    pub fn register_native(&self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        self.register(tool, ToolSource::Native, ToolPermission::Auto)
    }

    /// Register a native tool with a factory that rebuilds it per request.
    pub fn register_native_with_factory(
        &self,
        tool: Arc<dyn Tool>,
        factory: ToolFactory,
    ) -> Result<(), ToolError> {
        self.register_with_factory(tool, factory, ToolSource::Native, ToolPermission::Auto)
    }

    /// Register a CLI tool with its config and default permission.
    pub fn register_cli(&self, config: CliToolConfig) -> Result<(), ToolError> {
        let permission = config.permission;
        let tool = Arc::new(CliTool::new(config.clone()));
        self.register_with_cli_config(tool, ToolSource::Cli, permission, Some(config))
    }

    /// Register a tool.
    pub fn register(
        &self,
        tool: Arc<dyn Tool>,
        source: ToolSource,
        permission: ToolPermission,
    ) -> Result<(), ToolError> {
        self.register_with_cli_config(tool, source, permission, None)
    }

    /// Register a tool with a factory that rebuilds it per request.
    pub fn register_with_factory(
        &self,
        tool: Arc<dyn Tool>,
        factory: ToolFactory,
        source: ToolSource,
        permission: ToolPermission,
    ) -> Result<(), ToolError> {
        self.register_with_cli_config_and_factory(tool, source, permission, None, Some(factory))
    }

    fn register_with_cli_config(
        &self,
        tool: Arc<dyn Tool>,
        source: ToolSource,
        permission: ToolPermission,
        cli_config: Option<CliToolConfig>,
    ) -> Result<(), ToolError> {
        self.register_with_cli_config_and_factory(tool, source, permission, cli_config, None)
    }

    fn register_with_cli_config_and_factory(
        &self,
        tool: Arc<dyn Tool>,
        source: ToolSource,
        permission: ToolPermission,
        cli_config: Option<CliToolConfig>,
        factory: Option<ToolFactory>,
    ) -> Result<(), ToolError> {
        let mut entries = self.entries.write().unwrap();
        let name = tool.name().to_string();
        if entries.contains_key(&name) {
            return Err(ToolError::already_registered(&name));
        }
        let display_name = {
            let dn = tool.display_name();
            if dn == "Unnamed Tool" {
                super::snake_to_title_case(tool.name())
            } else {
                dn.to_string()
            }
        };
        let metadata = ToolMetadata {
            name: name.clone(),
            display_name,
            description: tool.description().to_string(),
            source,
            permission,
            is_write_tool: tool.is_write_tool(),
        };
        entries.insert(
            name,
            ToolEntry {
                tool,
                metadata,
                cli_config,
                factory,
            },
        );
        Ok(())
    }

    /// Retrieve a tool by name.
    pub fn get(&self, name: &str) -> Option<(Arc<dyn Tool>, ToolMetadata)> {
        let entries = self.entries.read().unwrap();
        entries
            .get(name)
            .map(|entry| (Arc::clone(&entry.tool), entry.metadata.clone()))
    }

    /// Retrieve metadata for a tool by name.
    pub fn metadata(&self, name: &str) -> Option<ToolMetadata> {
        let entries = self.entries.read().unwrap();
        entries.get(name).map(|entry| entry.metadata.clone())
    }

    /// Set the permission for a tool.
    pub fn set_permission(&self, name: &str, permission: ToolPermission) -> Result<(), ToolError> {
        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .get_mut(name)
            .ok_or_else(|| ToolError::not_found(name))?;
        entry.metadata.permission = permission;
        Ok(())
    }

    /// List all registered tools with metadata.
    pub fn list(&self) -> Vec<ToolMetadata> {
        let entries = self.entries.read().unwrap();
        entries.values().map(|e| e.metadata.clone()).collect()
    }

    /// Export all tools in OpenAI-compatible function-calling format.
    /// Disabled tools are skipped so the model does not see them.
    pub fn export_openai_tools(&self) -> Vec<Value> {
        self.export_openai_tools_filtered(true)
    }

    /// Export tools, optionally excluding write-capable tools.
    ///
    /// When `allow_write_tools` is `false` (incognito turns), write-capable
    /// tools such as `remember` are omitted so the LLM cannot persist facts
    /// (issue #155). Disabled tools are always skipped.
    pub fn export_openai_tools_filtered(&self, allow_write_tools: bool) -> Vec<Value> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|entry| entry.metadata.permission != ToolPermission::Disabled)
            .filter(|entry| allow_write_tools || !entry.metadata.is_write_tool)
            .map(|entry| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": entry.metadata.name,
                        "description": entry.metadata.description,
                        "parameters": entry.tool.parameters_schema(),
                        "strict": true,
                    }
                })
            })
            .collect()
    }

    /// Export tools ready for the LLM backend.
    /// Returns `None` when there are no enabled tools so the request omits the field.
    pub fn export_openai_tools_for_llm(&self) -> Option<Vec<Value>> {
        self.export_openai_tools_for_llm_with_writes(true)
    }

    /// Like [`Self::export_openai_tools_for_llm`] but honours the incognito
    /// contract: write-capable tools are suppressed when `allow_write_tools`
    /// is `false` (issue #155).
    pub fn export_openai_tools_for_llm_with_writes(
        &self,
        allow_write_tools: bool,
    ) -> Option<Vec<Value>> {
        let tools = self.export_openai_tools_filtered(allow_write_tools);
        if tools.is_empty() { None } else { Some(tools) }
    }

    /// Return whether the named tool mutates persistent state (issue #155).
    /// Unknown tools are treated as non-write so reads are never accidentally
    /// blocked.
    pub fn is_write_tool(&self, name: &str) -> bool {
        self.metadata(name).is_some_and(|m| m.is_write_tool)
    }

    /// Look up the display name for a tool by name.
    pub fn get_display_name(&self, name: &str) -> Option<String> {
        self.metadata(name).map(|m| m.display_name)
    }

    /// Execute a tool by name with the given JSON arguments and per-request context.
    ///
    /// Applies the registry's uniform checks — the incognito write-tool
    /// guard, the permission level (Auto/Ask/Disabled), and factory
    /// resolution — before invoking the tool (issue #441).
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let (is_write_tool, permission, factory, tool) = {
            let entries = self.entries.read().unwrap();
            let entry = entries
                .get(name)
                .ok_or_else(|| ToolError::not_found(name))?;
            (
                entry.metadata.is_write_tool,
                entry.metadata.permission,
                entry.factory.clone(),
                Arc::clone(&entry.tool),
            )
        };

        if !ctx.allow_write_tools && is_write_tool {
            return Err(ToolError::blocked_incognito(name));
        }

        match permission {
            ToolPermission::Disabled => return Err(ToolError::disabled(name)),
            ToolPermission::Ask => return Err(ToolError::permission_denied(name)),
            ToolPermission::Auto => {}
        }

        let tool = match factory {
            Some(factory) => factory(ctx),
            None => tool,
        };
        tool.execute(args).await
    }

    /// Load CLI tool definitions and permission overrides from a TOML file.
    pub fn load_tools_config(&self, path: &Path) -> Result<(), ToolError> {
        let config = super::ToolsConfig::load(path)?;

        for cli_config in config.tools {
            self.register_cli(cli_config)?;
        }

        for (name, permission) in config.permissions {
            self.set_permission(&name, permission)?;
        }

        Ok(())
    }

    /// Save current CLI tool definitions and permission overrides to a TOML file.
    pub fn save_tools_config(&self, path: &Path) -> Result<(), ToolError> {
        let entries = self.entries.read().unwrap();

        let mut tools = Vec::new();
        let mut permissions = HashMap::new();

        for entry in entries.values() {
            if let Some(ref cli) = entry.cli_config {
                let mut cli = cli.clone();
                // Update the stored permission to match current metadata.
                cli.permission = entry.metadata.permission;
                tools.push(cli);
            }
            // Store permission for all tools.
            if entry.metadata.permission != ToolPermission::Auto
                || entry.metadata.source == ToolSource::Cli
            {
                permissions.insert(entry.metadata.name.clone(), entry.metadata.permission);
            }
        }

        let config = super::ToolsConfig { tools, permissions };
        drop(entries);
        config.save(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmClient;
    use async_trait::async_trait;

    /// Test tool that reports the LLM backend it was built with.
    struct LlmReportingTool {
        llm: Arc<dyn LlmBackend>,
    }

    #[async_trait]
    impl Tool for LlmReportingTool {
        fn name(&self) -> &str {
            "llm_reporting"
        }

        fn description(&self) -> &str {
            "reports the LLM backend it was built with"
        }

        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        fn permission(&self) -> ToolPermission {
            ToolPermission::Auto
        }

        async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                result: Some(serde_json::json!(format!("{:?}", self.llm))),
                ..Default::default()
            })
        }
    }

    /// Test write-capable tool for the incognito guard.
    struct WriteTool;

    #[async_trait]
    impl Tool for WriteTool {
        fn name(&self) -> &str {
            "write_tool"
        }

        fn description(&self) -> &str {
            "writes state"
        }

        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        fn permission(&self) -> ToolPermission {
            ToolPermission::Auto
        }

        async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::default())
        }

        fn is_write_tool(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn factory_tool_executes_with_request_context() {
        let registry = ToolRegistry::new();
        let prototype_llm: Arc<dyn LlmBackend> = Arc::new(MockLlmClient::builder().build());
        let request_llm: Arc<dyn LlmBackend> = Arc::new(
            MockLlmClient::builder()
                .push_chat("queued", crate::llm::Usage::default())
                .build(),
        );
        registry
            .register_native_with_factory(
                Arc::new(LlmReportingTool {
                    llm: Arc::clone(&prototype_llm),
                }),
                Arc::new(|ctx: &ToolContext| {
                    Arc::new(LlmReportingTool {
                        llm: Arc::clone(&ctx.llm),
                    })
                }),
            )
            .unwrap();

        let output = registry
            .execute(
                "llm_reporting",
                serde_json::json!({}),
                &ToolContext::new(Arc::clone(&request_llm), true),
            )
            .await
            .unwrap();

        let reported = output.result.expect("tool should report its LLM");
        assert_eq!(reported, serde_json::json!(format!("{:?}", request_llm)));
        assert_ne!(reported, serde_json::json!(format!("{:?}", prototype_llm)));
    }

    #[tokio::test]
    async fn factory_tool_respects_permission_checks() {
        let llm: Arc<dyn LlmBackend> = Arc::new(MockLlmClient::builder().build());
        let factory: ToolFactory = Arc::new(|ctx: &ToolContext| {
            Arc::new(LlmReportingTool {
                llm: Arc::clone(&ctx.llm),
            })
        });

        let disabled = ToolRegistry::new();
        disabled
            .register_with_factory(
                Arc::new(LlmReportingTool {
                    llm: Arc::clone(&llm),
                }),
                Arc::clone(&factory),
                ToolSource::Native,
                ToolPermission::Disabled,
            )
            .unwrap();
        assert_eq!(
            disabled
                .execute(
                    "llm_reporting",
                    serde_json::json!({}),
                    &ToolContext::new(Arc::clone(&llm), true)
                )
                .await
                .unwrap_err(),
            ToolError::disabled("llm_reporting")
        );

        let ask = ToolRegistry::new();
        ask.register_with_factory(
            Arc::new(LlmReportingTool {
                llm: Arc::clone(&llm),
            }),
            factory,
            ToolSource::Native,
            ToolPermission::Ask,
        )
        .unwrap();
        assert_eq!(
            ask.execute(
                "llm_reporting",
                serde_json::json!({}),
                &ToolContext::new(Arc::clone(&llm), true)
            )
            .await
            .unwrap_err(),
            ToolError::permission_denied("llm_reporting")
        );
    }

    #[tokio::test]
    async fn factory_write_tool_is_blocked_in_incognito() {
        let registry = ToolRegistry::new();
        let llm: Arc<dyn LlmBackend> = Arc::new(MockLlmClient::builder().build());
        registry
            .register_native_with_factory(
                Arc::new(WriteTool),
                Arc::new(|_ctx: &ToolContext| Arc::new(WriteTool)),
            )
            .unwrap();

        let err = registry
            .execute(
                "write_tool",
                serde_json::json!({}),
                &ToolContext::new(Arc::clone(&llm), false),
            )
            .await
            .unwrap_err();
        assert_eq!(err, ToolError::blocked_incognito("write_tool"));
    }

    #[tokio::test]
    async fn non_factory_tool_executes_with_context() {
        let registry = ToolRegistry::with_builtins();
        let llm: Arc<dyn LlmBackend> = Arc::new(MockLlmClient::builder().build());
        let output = registry
            .execute(
                "echo",
                serde_json::json!({"message": "hi"}),
                &ToolContext::new(llm, true),
            )
            .await
            .unwrap();
        assert!(output.to_display_text().contains("hi"));
    }
}
