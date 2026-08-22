//! Configuration loading, persistence, and platform path resolution.

use std::path::{Path, PathBuf};

use crate::config::types::*;
use crate::paths;

impl Config {
    /// Load configuration, applying the precedence:
    /// 1. Compiled defaults
    /// 2. Auto-initialised directories and default config (if no file exists)
    /// 3. TOML file (optional path or default location)
    /// 4. `MIMIR_*` environment variables
    ///
    /// If `path` is `Some`, the file must exist or an error is returned.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut config = Config::default();

        match path {
            Some(p) => {
                let contents = std::fs::read_to_string(p)?;
                config = toml::from_str(&contents)?;
            }
            None => {
                let config_path = paths::config_path()?;
                if config_path.exists() {
                    let contents = std::fs::read_to_string(&config_path)?;
                    config = toml::from_str(&contents)?;
                } else {
                    // First run: bootstrap directories and write default config.
                    Self::init()?;
                }
            }
        }

        config.apply_env_overrides();
        Ok(config)
    }

    /// Persist the current configuration to `path` as pretty-printed TOML.
    ///
    /// Parent directories are created automatically.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml = toml::to_string_pretty(self)?;
        std::fs::write(path, toml)?;
        Ok(())
    }

    /// Return the default platform configuration file path.
    pub fn config_path() -> Option<PathBuf> {
        paths::config_path().ok()
    }
}
