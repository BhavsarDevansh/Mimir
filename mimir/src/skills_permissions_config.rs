use mimir_core::tools::ToolPermission;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Skill permission overrides stored on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsPermissionsConfig {
    #[serde(default)]
    pub permissions: HashMap<String, ToolPermission>,
}

impl SkillsPermissionsConfig {
    /// Load from a TOML file path.
    pub fn load(path: &Path) -> Result<Self, mimir_core::skills::SkillError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            mimir_core::skills::SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to read {path:?}: {e}"),
            )
        })?;
        let config: SkillsPermissionsConfig = toml::from_str(&content).map_err(|e| {
            mimir_core::skills::SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to parse skills permissions: {e}"),
            )
        })?;
        Ok(config)
    }

    /// Save to a TOML file path.
    pub fn save(&self, path: &Path) -> Result<(), mimir_core::skills::SkillError> {
        let content = toml::to_string_pretty(self).map_err(|e| {
            mimir_core::skills::SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to serialize skills permissions: {e}"),
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                mimir_core::skills::SkillError::execution_failed(
                    "skills_permissions_config",
                    format!("failed to create config dir {parent:?}: {e}"),
                )
            })?;
        }
        std::fs::write(path, content).map_err(|e| {
            mimir_core::skills::SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to write {path:?}: {e}"),
            )
        })?;
        Ok(())
    }

    /// Default path: `~/.config/mimir/skills_permissions.toml`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("mimir").join("skills_permissions.toml"))
    }
}
