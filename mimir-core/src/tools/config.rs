use super::{CliToolConfig, ToolPermission};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Full tools configuration file structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// CLI tool definitions (array-of-tables).
    #[serde(default, rename = "tool")]
    pub tools: Vec<CliToolConfig>,
    /// Permission overrides for any registered tool.
    #[serde(default)]
    pub permissions: HashMap<String, ToolPermission>,
}

impl ToolsConfig {
    /// Load from a TOML file path.
    pub fn load(path: &Path) -> Result<Self, super::ToolError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            super::ToolError::execution_failed(
                "tools_config",
                format!("failed to read {path:?}: {e}"),
            )
        })?;
        let config: ToolsConfig = toml::from_str(&content).map_err(|e| {
            super::ToolError::execution_failed(
                "tools_config",
                format!("failed to parse tools.toml: {e}"),
            )
        })?;
        Ok(config)
    }

    /// Save to a TOML file path.
    pub fn save(&self, path: &Path) -> Result<(), super::ToolError> {
        let content = toml::to_string_pretty(self).map_err(|e| {
            super::ToolError::execution_failed(
                "tools_config",
                format!("failed to serialize tools.toml: {e}"),
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                super::ToolError::execution_failed(
                    "tools_config",
                    format!("failed to create config dir {parent:?}: {e}"),
                )
            })?;
        }
        std::fs::write(path, content).map_err(|e| {
            super::ToolError::execution_failed(
                "tools_config",
                format!("failed to write {path:?}: {e}"),
            )
        })?;
        Ok(())
    }

    /// Default path: `~/.config/mimir/tools.toml`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("mimir").join("tools.toml"))
    }
}
