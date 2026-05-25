use super::{CliTool, CliToolConfig, Tool, ToolError, ToolOutput, ToolPermission};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

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
    pub description: String,
    pub source: ToolSource,
    pub permission: ToolPermission,
}

/// Entry in the registry combining the tool implementation with metadata.
pub struct ToolEntry {
    pub tool: Arc<dyn Tool>,
    pub metadata: ToolMetadata,
    /// Original CLI config, if this is a CLI tool.
    pub cli_config: Option<CliToolConfig>,
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
    }

    /// Register a native tool with its default permission.
    pub fn register_native(&self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        self.register(tool, ToolSource::Native, ToolPermission::Auto)
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

    fn register_with_cli_config(
        &self,
        tool: Arc<dyn Tool>,
        source: ToolSource,
        permission: ToolPermission,
        cli_config: Option<CliToolConfig>,
    ) -> Result<(), ToolError> {
        let mut entries = self.entries.write().unwrap();
        let name = tool.name().to_string();
        if entries.contains_key(&name) {
            return Err(ToolError::already_registered(&name));
        }
        let metadata = ToolMetadata {
            name: name.clone(),
            description: tool.description().to_string(),
            source,
            permission,
        };
        entries.insert(
            name,
            ToolEntry {
                tool,
                metadata,
                cli_config,
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
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|entry| entry.metadata.permission != ToolPermission::Disabled)
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
        let tools = self.export_openai_tools();
        if tools.is_empty() { None } else { Some(tools) }
    }

    /// Execute a tool by name with the given JSON arguments.
    pub async fn execute(&self, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
        let (tool, metadata) = self.get(name).ok_or_else(|| ToolError::not_found(name))?;

        match metadata.permission {
            ToolPermission::Disabled => return Err(ToolError::disabled(name)),
            ToolPermission::Ask => return Err(ToolError::permission_denied(name)),
            ToolPermission::Auto => {}
        }

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
