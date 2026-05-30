use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use crate::paths;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

/// Top-level configuration for Mimir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub llm: LlmConfig,
    pub agent: AgentConfig,
    pub memory: MemoryConfig,
    pub context: ContextConfig,
    pub personality: PersonalityConfig,
    pub server: ServerConfig,
}

/// Result of an initialisation attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum InitResult {
    /// All artefacts were created fresh.
    Created {
        /// Path to the configuration directory.
        config_dir: PathBuf,
        /// Path to the data directory.
        data_dir: PathBuf,
        /// Path to the generated default config file.
        config_file: PathBuf,
    },
    /// Everything already existed; nothing was written.
    AlreadyInitialized,
}

impl InitResult {
    /// Returns `true` if this result indicates new files were created.
    pub fn is_created(&self) -> bool {
        matches!(self, InitResult::Created { .. })
    }
}

/// LLM provider settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub temperature: f32,
}

/// Agent behaviour settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub name: String,
    pub proactivity: Proactivity,
    pub verbose_reasoning: bool,
    /// Maximum number of agentic tool-call rounds before forcing a final response.
    pub max_tool_rounds: u16,
}

/// Memory subsystem settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub enabled: bool,
    pub char_limit: u16,
    pub auto_manage: bool,
    pub temporal_horizon: u8,
}

/// Conversation context manager settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub max_turns: u16,
    pub db_path: Option<PathBuf>,
}

/// Personality subsystem settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersonalityConfig {
    pub preset: String,
}

/// Server (daemon) settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// TCP bind address for the HTTP server (e.g. "127.0.0.1:8080").
    pub bind_addr: String,
    /// Path to the Unix domain socket for local CLI communication.
    /// Set to None to disable Unix socket.
    /// Defaults to None (auto-detected from data dir on Unix platforms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".to_string(),
            socket_path: None,
        }
    }
}

/// How eagerly the agent initiates actions on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Proactivity {
    /// Never act without an explicit user request.
    Never,
    /// Only surface high-importance observations.
    #[default]
    ImportantOnly,
    /// Proactively suggest and act whenever useful.
    Always,
}

impl fmt::Display for Proactivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Proactivity::Never => write!(f, "never"),
            Proactivity::ImportantOnly => write!(f, "important_only"),
            Proactivity::Always => write!(f, "always"),
        }
    }
}

impl FromStr for Proactivity {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "never" => Ok(Proactivity::Never),
            "important_only" => Ok(Proactivity::ImportantOnly),
            "always" => Ok(Proactivity::Always),
            _ => Err(ConfigError::InvalidProactivity(s.to_string())),
        }
    }
}

/// Errors that can occur while loading or saving configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The TOML file could not be parsed.
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),

    /// A path resolution error occurred.
    #[error("Path error: {0}")]
    Paths(#[from] paths::PathsError),

    /// The supplied proactivity value is not recognised.
    #[error("Invalid proactivity value: '{0}'. Expected 'never', 'important_only', or 'always'.")]
    InvalidProactivity(String),
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            max_tokens: None,
            temperature: 0.2,
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "Mimir".to_string(),
            proactivity: Proactivity::ImportantOnly,
            verbose_reasoning: false,
            max_tool_rounds: 100,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            path: None,
            enabled: true,
            char_limit: 2500,
            auto_manage: true,
            temporal_horizon: 30,
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: None,
            max_turns: 20,
            db_path: paths::default_db_path().ok(),
        }
    }
}

impl Default for PersonalityConfig {
    fn default() -> Self {
        Self {
            preset: "transparent".to_string(),
        }
    }
}

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

    /// Initialise the Mimir environment: create config and data directories,
    /// write a default `config.toml` if it does not already exist.
    ///
    /// This is idempotent — subsequent calls return `AlreadyInitialized`
    /// without overwriting existing files.
    pub fn init() -> Result<InitResult, ConfigError> {
        let cfg_dir = paths::config_dir()?;
        let dat_dir = paths::data_dir()?;

        paths::ensure_dir(&cfg_dir)?;
        paths::ensure_dir(dat_dir.as_path())?;

        let cfg_path = cfg_dir.join("config.toml");

        // Use create_new for atomic "write only if not exists" semantics.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&cfg_path)
        {
            Ok(mut file) => {
                use std::io::Write;
                let default_toml = Self::default_config_toml();
                if let Err(e) = file.write_all(default_toml.as_bytes()) {
                    let _ = std::fs::remove_file(&cfg_path);
                    return Err(ConfigError::Io(e));
                }
                tracing::info!("Created default config at {}", cfg_path.display());
                Ok(InitResult::Created {
                    config_dir: cfg_dir,
                    data_dir: dat_dir,
                    config_file: cfg_path,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(InitResult::AlreadyInitialized)
            }
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    /// Generate the default `config.toml` contents with helpful comments.
    fn default_config_toml() -> String {
        r#"# Mimir Configuration
# Edit this file to customise Mimir's behaviour.
# You can also override any setting with MIMIR_* environment variables.

[llm]
endpoint = "https://api.openai.com/v1"
# Set your API key here, or use the MIMIR_LLM_API_KEY environment variable.
api_key = ""
model = "gpt-4o"
temperature = 0.2
# max_tokens = 4096  # Optional: limit tokens per generation

[agent]
name = "Mimir"
proactivity = "important_only"
verbose_reasoning = false

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
# path = "${CONFIG_DIR}/memory.md"  # Optional: override memory file location

[context]
max_turns = 20
# db_path is resolved automatically; override only if needed.
# db_path = "${USER_DATA_DIR}/mimir/context.db"
# max_tokens = 4096  # Optional: token budget for conversation history

[personality]
preset = "transparent"

[server]
bind_addr = "127.0.0.1:8080"
# socket_path = "~/.local/share/mimir/mimir.sock"  # Optional: Unix domain socket for local CLI
"#
        .to_string()
    }

    /// Apply environment variable overrides.
    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("MIMIR_LLM_ENDPOINT") {
            self.llm.endpoint = v;
        }
        if let Ok(v) = std::env::var("MIMIR_LLM_API_KEY") {
            self.llm.api_key = v;
        }
        if let Ok(v) = std::env::var("MIMIR_LLM_MODEL") {
            self.llm.model = v;
        }
        if let Ok(v) = std::env::var("MIMIR_LLM_MAX_TOKENS")
            && let Ok(n) = v.parse::<u32>()
        {
            self.llm.max_tokens = Some(n);
        }
        if let Ok(v) = std::env::var("MIMIR_LLM_TEMPERATURE")
            && let Ok(n) = v.parse::<f32>()
        {
            self.llm.temperature = n;
        }
        if let Ok(v) = std::env::var("MIMIR_AGENT_NAME") {
            self.agent.name = v;
        }
        if let Ok(v) = std::env::var("MIMIR_AGENT_PROACTIVITY")
            && let Ok(p) = Proactivity::from_str(&v)
        {
            self.agent.proactivity = p;
        }
        if let Ok(v) = std::env::var("MIMIR_AGENT_VERBOSE_REASONING")
            && let Ok(b) = v.parse::<bool>()
        {
            self.agent.verbose_reasoning = b;
        }
        if let Ok(v) = std::env::var("MIMIR_MEMORY_ENABLED")
            && let Ok(b) = v.parse::<bool>()
        {
            self.memory.enabled = b;
        }
        if let Ok(v) = std::env::var("MIMIR_MEMORY_CHAR_LIMIT")
            && let Ok(n) = v.parse::<u16>()
        {
            self.memory.char_limit = n;
        }
        if let Ok(v) = std::env::var("MIMIR_MEMORY_AUTO_MANAGE")
            && let Ok(b) = v.parse::<bool>()
        {
            self.memory.auto_manage = b;
        }
        if let Ok(v) = std::env::var("MIMIR_MEMORY_TEMPORAL_HORIZON")
            && let Ok(n) = v.parse::<u8>()
        {
            self.memory.temporal_horizon = n;
        }
        if let Ok(v) = std::env::var("MIMIR_MEMORY_PATH")
            && !v.trim().is_empty()
        {
            self.memory.path = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("MIMIR_CONTEXT_MAX_TOKENS")
            && let Ok(n) = v.parse::<u32>()
        {
            self.context.max_tokens = Some(n);
        }
        if let Ok(v) = std::env::var("MIMIR_CONTEXT_MAX_TURNS")
            && let Ok(n) = v.parse::<u16>()
        {
            self.context.max_turns = n;
        }
        if let Ok(v) = std::env::var("MIMIR_CONTEXT_DB_PATH") {
            self.context.db_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("MIMIR_PERSONALITY_PRESET") {
            self.personality.preset = v;
        }
        if let Ok(v) = std::env::var("MIMIR_SERVER_BIND_ADDR") {
            self.server.bind_addr = v;
        }
        if let Ok(v) = std::env::var("MIMIR_SERVER_SOCKET_PATH") {
            self.server.socket_path = if v.trim().is_empty() { None } else { Some(v) };
        }
    }
}

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
    use serial_test::serial;
    use std::io::Write;

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.memory.char_limit, 2500);
        assert_eq!(config.memory.path, None);
        assert_eq!(config.llm.max_tokens, None);
        assert_eq!(config.context.max_tokens, None);
        assert_eq!(config.context.max_turns, 20);
        assert_eq!(config.context.db_path, paths::default_db_path().ok());
        assert_eq!(config.server.bind_addr, "127.0.0.1:8080");
        assert_eq!(config.server.socket_path, None);
    }

    #[test]
    #[serial]
    fn test_env_override_llm() {
        unsafe {
            std::env::set_var("MIMIR_LLM_MODEL", "gpt-3.5-turbo");
        }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.llm.model, "gpt-3.5-turbo");
        unsafe {
            std::env::remove_var("MIMIR_LLM_MODEL");
        }
    }

    #[test]
    #[serial]
    fn test_env_override_context() {
        unsafe {
            std::env::set_var("MIMIR_CONTEXT_MAX_TOKENS", "8192");
            std::env::set_var("MIMIR_CONTEXT_MAX_TURNS", "50");
            std::env::set_var("MIMIR_CONTEXT_DB_PATH", "/tmp/mimir/context.db");
        }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.context.max_tokens, Some(8192));
        assert_eq!(config.context.max_turns, 50);
        assert_eq!(
            config.context.db_path,
            Some(PathBuf::from("/tmp/mimir/context.db"))
        );
        unsafe {
            std::env::remove_var("MIMIR_CONTEXT_MAX_TOKENS");
            std::env::remove_var("MIMIR_CONTEXT_MAX_TURNS");
            std::env::remove_var("MIMIR_CONTEXT_DB_PATH");
        }
    }

    #[test]
    #[serial]
    fn test_env_override_memory_path() {
        unsafe {
            std::env::set_var("MIMIR_MEMORY_PATH", "/tmp/mimir/memory.md");
        }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(
            config.memory.path,
            Some(PathBuf::from("/tmp/mimir/memory.md"))
        );
        unsafe {
            std::env::remove_var("MIMIR_MEMORY_PATH");
        }
    }

    #[test]
    #[serial]
    fn test_env_override_memory_path_blank_is_ignored() {
        unsafe {
            std::env::set_var("MIMIR_MEMORY_PATH", "   ");
        }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.memory.path, None);
        unsafe {
            std::env::remove_var("MIMIR_MEMORY_PATH");
        }
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let original = Config {
            llm: LlmConfig {
                endpoint: "http://localhost:8080".to_string(),
                api_key: "secret".to_string(),
                model: "test-model".to_string(),
                max_tokens: Some(100),
                temperature: 0.5,
            },
            agent: AgentConfig {
                name: "TestAgent".to_string(),
                proactivity: Proactivity::Always,
                verbose_reasoning: true,
                max_tool_rounds: 100,
            },
            memory: MemoryConfig {
                path: None,
                enabled: false,
                char_limit: 100,
                auto_manage: false,
                temporal_horizon: 7,
            },
            context: ContextConfig {
                max_tokens: Some(2048),
                max_turns: 10,
                db_path: Some(PathBuf::from("~/.local/share/mimir/context.db")),
            },
            personality: PersonalityConfig {
                preset: "transparent".to_string(),
            },
            server: ServerConfig {
                bind_addr: "127.0.0.1:8080".to_string(),
                socket_path: None,
            },
        };

        original.save(&path).unwrap();
        let loaded = Config::load(Some(&path)).unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn test_toml_roundtrip() {
        let toml_str = r#"
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"
max_tokens = 4096
temperature = 0.2

[agent]
name = "Mimir"
proactivity = "important_only"
verbose_reasoning = false

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
# path = "${CONFIG_DIR}/memory.md"  # Optional: override memory file location

[context]
max_tokens = 4096
max_turns = 20
db_path = "~/.local/share/mimir/context.db"
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.llm.max_tokens, Some(4096));
        assert_eq!(config.context.max_tokens, Some(4096));
        assert_eq!(config.context.max_turns, 20);
        assert_eq!(
            config.context.db_path,
            Some(PathBuf::from("~/.local/share/mimir/context.db"))
        );
    }

    #[test]
    fn test_proactivity_from_str() {
        assert_eq!(Proactivity::from_str("never").unwrap(), Proactivity::Never);
        assert_eq!(
            Proactivity::from_str("IMPORTANT_ONLY").unwrap(),
            Proactivity::ImportantOnly
        );
        assert_eq!(
            Proactivity::from_str("Always").unwrap(),
            Proactivity::Always
        );
        assert!(Proactivity::from_str("invalid").is_err());
    }

    #[test]
    fn test_proactivity_display() {
        assert_eq!(Proactivity::Never.to_string(), "never");
        assert_eq!(Proactivity::ImportantOnly.to_string(), "important_only");
        assert_eq!(Proactivity::Always.to_string(), "always");
    }

    #[test]
    fn test_missing_config_file_errors_when_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = Config::load(Some(&path));
        assert!(
            result.is_err(),
            "explicit non-existent path should return an error"
        );
    }

    #[test]
    #[serial]
    fn test_load_none_uses_defaults_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        // Save original XDG_CONFIG_HOME and override it to temp dir.
        let orig = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", home);
        }

        let config = Config::load(None).unwrap();
        assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.memory.char_limit, 2500);
        assert_eq!(config.memory.path, None);
        assert_eq!(config.llm.max_tokens, None);
        assert_eq!(config.context.max_tokens, None);
        assert_eq!(config.context.max_turns, 20);

        // Restore original state.
        match orig {
            Some(val) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", val);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
    }

    #[test]
    fn test_config_path_returns_platform_path() {
        let path = Config::config_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.ends_with("mimir/config.toml"));
    }

    #[test]
    #[serial]
    fn test_invalid_proactivity_env_ignored() {
        unsafe {
            std::env::set_var("MIMIR_AGENT_PROACTIVITY", "nonsense");
        }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.agent.proactivity, Proactivity::ImportantOnly);
        unsafe {
            std::env::remove_var("MIMIR_AGENT_PROACTIVITY");
        }
    }

    #[test]
    fn test_load_from_toml_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        {
            let mut file = std::fs::File::create(&path).unwrap();
            writeln!(
                file,
                r#"
[llm]
model = "custom-model"

[context]
max_turns = 5
"#
            )
            .unwrap();
        }

        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.llm.model, "custom-model");
        assert_eq!(config.context.max_turns, 5);
        assert_eq!(config.context.max_tokens, None); // default
    }

    #[test]
    fn test_personality_config_toml_roundtrip() {
        let toml_str = r#"
[personality]
preset = "formal"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.personality.preset, "formal");
    }

    #[test]
    #[serial]
    fn test_personality_preset_env_override() {
        unsafe {
            std::env::set_var("MIMIR_PERSONALITY_PRESET", "concise");
        }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.personality.preset, "concise");
        unsafe {
            std::env::remove_var("MIMIR_PERSONALITY_PRESET");
        }
    }

    #[test]
    #[serial]
    fn test_init_creates_config_dir_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");

        let orig_cfg = std::env::var_os("XDG_CONFIG_HOME");
        let orig_data = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &cfg_home);
            std::env::set_var("XDG_DATA_HOME", &data_home);
        }

        let result = Config::init().unwrap();
        match result {
            InitResult::Created {
                config_dir,
                data_dir,
                config_file,
            } => {
                assert!(config_dir.exists());
                assert!(data_dir.exists());
                assert!(config_file.exists());
                assert!(config_file.ends_with("config.toml"));
            }
            InitResult::AlreadyInitialized => {
                panic!("first init should report Created");
            }
        }

        // Verify config.toml content is valid TOML.
        let contents = std::fs::read_to_string(cfg_home.join("mimir").join("config.toml")).unwrap();
        let parsed: Config = toml::from_str(&contents).unwrap();
        assert_eq!(parsed.llm.model, "gpt-4o");

        match orig_cfg {
            Some(v) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        match orig_data {
            Some(v) => unsafe {
                std::env::set_var("XDG_DATA_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
    }

    #[test]
    #[serial]
    fn test_init_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");

        let orig_cfg = std::env::var_os("XDG_CONFIG_HOME");
        let orig_data = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &cfg_home);
            std::env::set_var("XDG_DATA_HOME", &data_home);
        }

        let result1 = Config::init().unwrap();
        assert!(matches!(result1, InitResult::Created { .. }));

        let result2 = Config::init().unwrap();
        assert!(matches!(result2, InitResult::AlreadyInitialized));

        // Config file should not have been overwritten.
        let contents = std::fs::read_to_string(cfg_home.join("mimir").join("config.toml")).unwrap();
        // Default TOML should still parse cleanly.
        let parsed: Config = toml::from_str(&contents).unwrap();
        assert_eq!(parsed.llm.model, "gpt-4o");

        match orig_cfg {
            Some(v) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        match orig_data {
            Some(v) => unsafe {
                std::env::set_var("XDG_DATA_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
    }

    #[test]
    #[serial]
    fn test_load_none_bootstraps_on_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");

        let orig_cfg = std::env::var_os("XDG_CONFIG_HOME");
        let orig_data = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &cfg_home);
            std::env::set_var("XDG_DATA_HOME", &data_home);
        }

        // No config file exists yet.
        let cfg_path = cfg_home.join("mimir").join("config.toml");
        assert!(!cfg_path.exists());

        let config = Config::load(None).unwrap();

        // Config file should now exist on disk.
        assert!(cfg_path.exists());

        // Returned config should be defaults.
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.name, "Mimir");

        match orig_cfg {
            Some(v) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        match orig_data {
            Some(v) => unsafe {
                std::env::set_var("XDG_DATA_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
    }

    #[test]
    fn test_default_config_toml_is_valid() {
        let toml_str = Config::default_config_toml();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.llm.endpoint, "https://api.openai.com/v1");
        assert_eq!(parsed.llm.api_key, "");
        assert_eq!(parsed.llm.model, "gpt-4o");
        assert_eq!(parsed.agent.name, "Mimir");
        assert_eq!(parsed.agent.max_tool_rounds, 100);
    }

    #[test]
    #[serial]
    fn test_init_does_not_overwrite_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");

        let orig_cfg = std::env::var_os("XDG_CONFIG_HOME");
        let orig_data = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &cfg_home);
            std::env::set_var("XDG_DATA_HOME", &data_home);
        }

        // Write a custom config first.
        Config::init().unwrap();
        let cfg_path = cfg_home.join("mimir").join("config.toml");
        let custom = r#"
[llm]
model = "custom-model"
"#;
        std::fs::write(&cfg_path, custom).unwrap();

        // init again — should not overwrite.
        let result = Config::init().unwrap();
        assert!(matches!(result, InitResult::AlreadyInitialized));

        let contents = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(contents.contains("custom-model"));

        match orig_cfg {
            Some(v) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        match orig_data {
            Some(v) => unsafe {
                std::env::set_var("XDG_DATA_HOME", v);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
    }
}

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
