use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::types::Config;

use thiserror::Error;
use tokio::sync::RwLock;

/// Errors that can occur while reloading configuration at runtime.
#[derive(Debug, Error)]
pub enum ConfigReloadError {
    /// An I/O error occurred while reading the config file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The TOML file could not be parsed.
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),

    /// A sensitive field was modified; reload is aborted.
    #[error("Sensitive field changed: {field}")]
    SensitiveFieldChanged { field: &'static str },
}

/// Live configuration that can be reloaded from disk without restarting.
///
/// Holds the current [`Config`] behind an `Arc<RwLock<Config>>` so that
/// readers can take a cheap snapshot while a reload is in progress.
#[derive(Debug, Clone)]
pub struct ReloadableConfig {
    inner: Arc<RwLock<Config>>,
    path: PathBuf,
}

impl ReloadableConfig {
    /// Create a new `ReloadableConfig` from an initial [`Config`] and its
    /// source file path.
    pub fn new(config: Config, path: PathBuf) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
            path,
        }
    }

    /// Return the config file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Clone the current configuration.
    ///
    /// The lock is held only for the clone, so this is safe to call across
    /// await points.
    pub async fn snapshot(&self) -> Config {
        self.inner.read().await.clone()
    }

    /// Reload the configuration from disk.
    ///
    /// 1. Read the file at `self.path`.
    /// 2. Parse it as TOML.
    /// 3. Compare sensitive fields (`llm.endpoint`, `llm.api_key`,
    ///    `llm.model`, `server.bind_addr`, `server.socket_path`) against the
    ///    current snapshot. If any differ, return
    ///    [`ConfigReloadError::SensitiveFieldChanged`] and leave the current
    ///    config untouched.
    /// 4. On success, write the new config into the lock and log.
    pub async fn reload(&self) -> Result<(), ConfigReloadError> {
        let contents = tokio::fs::read_to_string(&self.path).await?;
        let mut new_config: Config = toml::from_str(&contents)?;

        // Apply environment overrides to the new config before comparing sensitive fields.
        new_config.apply_env_overrides();
        new_config.normalise();

        let current = self.inner.read().await.clone();

        // Sensitive field gate.
        if current.llm.endpoint != new_config.llm.endpoint {
            return Err(ConfigReloadError::SensitiveFieldChanged {
                field: "llm.endpoint",
            });
        }
        if current.llm.api_key != new_config.llm.api_key {
            return Err(ConfigReloadError::SensitiveFieldChanged {
                field: "llm.api_key",
            });
        }
        if current.llm.model != new_config.llm.model {
            return Err(ConfigReloadError::SensitiveFieldChanged { field: "llm.model" });
        }
        if current.server.bind_addr != new_config.server.bind_addr {
            return Err(ConfigReloadError::SensitiveFieldChanged {
                field: "server.bind_addr",
            });
        }
        if current.server.socket_path != new_config.server.socket_path {
            return Err(ConfigReloadError::SensitiveFieldChanged {
                field: "server.socket_path",
            });
        }

        let mut write_guard = self.inner.write().await;
        *write_guard = new_config;
        drop(write_guard);

        tracing::info!("Config reloaded from {}", self.path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reloadable_applies_non_sensitive_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.personality.preset = "default".to_string();
        config.memory.char_limit = 1000;

        let toml_str = r#"
[personality]
preset = "default"

[memory]
char_limit = 1000

[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"

[server]
bind_addr = "127.0.0.1:8080"
"#
        .to_string();
        tokio::fs::write(&path, &toml_str).await.unwrap();

        let reloadable = ReloadableConfig::new(config, path.clone());

        // Modify non-sensitive fields.
        let new_toml = r#"
[personality]
preset = "concise"

[memory]
char_limit = 2000

[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"

[server]
bind_addr = "127.0.0.1:8080"
"#
        .to_string();
        tokio::fs::write(&path, &new_toml).await.unwrap();

        reloadable.reload().await.unwrap();

        let snapshot = reloadable.snapshot().await;
        assert_eq!(snapshot.personality.preset, "concise");
        assert_eq!(snapshot.memory.char_limit, 2000);
    }

    #[tokio::test]
    async fn test_reloadable_rejects_sensitive_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.llm.model = "gpt-4o".to_string();

        let toml_str = r#"
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"

[server]
bind_addr = "127.0.0.1:8080"
"#;
        tokio::fs::write(&path, toml_str).await.unwrap();

        let reloadable = ReloadableConfig::new(config, path.clone());

        let new_toml = r#"
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o-mini"

[server]
bind_addr = "127.0.0.1:8080"
"#;
        tokio::fs::write(&path, &new_toml).await.unwrap();

        let err = reloadable.reload().await.unwrap_err();
        assert!(
            matches!(err, ConfigReloadError::SensitiveFieldChanged { field } if field == "llm.model")
        );

        let snapshot = reloadable.snapshot().await;
        assert_eq!(snapshot.llm.model, "gpt-4o");
    }

    #[tokio::test]
    async fn test_reloadable_rejects_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::default();
        let toml_str = r#"
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"

[server]
bind_addr = "127.0.0.1:8080"
"#;
        tokio::fs::write(&path, toml_str).await.unwrap();

        let reloadable = ReloadableConfig::new(config, path.clone());

        tokio::fs::write(&path, "not valid toml [[").await.unwrap();

        let err = reloadable.reload().await.unwrap_err();
        assert!(matches!(err, ConfigReloadError::Parse(_)));

        let snapshot = reloadable.snapshot().await;
        assert_eq!(snapshot.llm.model, "gpt-4o");
    }
}
