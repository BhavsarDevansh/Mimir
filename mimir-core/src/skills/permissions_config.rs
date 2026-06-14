use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::tools::ToolPermission;

use super::SkillError;

/// Skill permission overrides stored on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsPermissionsConfig {
    #[serde(default)]
    pub permissions: HashMap<String, ToolPermission>,
}

impl SkillsPermissionsConfig {
    /// Load from a TOML file path.
    pub fn load(path: &Path) -> Result<Self, SkillError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to read {path:?}: {e}"),
            )
        })?;
        let config: SkillsPermissionsConfig = toml::from_str(&content).map_err(|e| {
            SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to parse skills permissions: {e}"),
            )
        })?;
        Ok(config)
    }

    /// Save to a TOML file path.
    pub fn save(&self, path: &Path) -> Result<(), SkillError> {
        let content = toml::to_string_pretty(self).map_err(|e| {
            SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to serialize skills permissions: {e}"),
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SkillError::execution_failed(
                    "skills_permissions_config",
                    format!("failed to create config dir {parent:?}: {e}"),
                )
            })?;
        }
        std::fs::write(path, content).map_err(|e| {
            SkillError::execution_failed(
                "skills_permissions_config",
                format!("failed to write {path:?}: {e}"),
            )
        })?;
        Ok(())
    }

    /// Default path: `~/.config/mimir/skills_permissions.toml`.
    pub fn default_path() -> Option<PathBuf> {
        paths::config_dir()
            .ok()
            .map(|d| d.join("skills_permissions.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_default_path_ends_with_skills_permissions_toml() {
        let path = SkillsPermissionsConfig::default_path();
        assert!(path.is_some());
        assert!(path.unwrap().ends_with("mimir/skills_permissions.toml"));
    }

    #[test]
    fn test_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skills_permissions.toml");

        let mut permissions = HashMap::new();
        permissions.insert("research_synthesis".to_string(), ToolPermission::Auto);
        permissions.insert(
            "test_driven_development".to_string(),
            ToolPermission::Disabled,
        );
        let original = SkillsPermissionsConfig { permissions };

        original.save(&path).unwrap();
        let loaded = SkillsPermissionsConfig::load(&path).unwrap();
        assert_eq!(loaded.permissions.len(), 2);
        assert_eq!(
            loaded.permissions.get("research_synthesis"),
            Some(&ToolPermission::Auto)
        );
        assert_eq!(
            loaded.permissions.get("test_driven_development"),
            Some(&ToolPermission::Disabled)
        );
    }

    #[test]
    fn test_load_invalid_toml_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skills_permissions.toml");
        let mut file = std::fs::File::create(&path).unwrap();
        write!(file, "not valid toml").unwrap();
        drop(file);

        let result = SkillsPermissionsConfig::load(&path);
        assert!(result.is_err());
    }
}
