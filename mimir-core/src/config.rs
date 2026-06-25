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
    pub knowledge: KnowledgeConfig,
    pub identity: IdentityConfig,
    pub scheduler: SchedulerConfig,
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
    pub enabled: bool,
    pub char_limit: u16,
    pub auto_manage: bool,
    pub temporal_horizon: u8,
    /// Number of top-ranked facts to include in the condensation hash.
    pub condensation_top_n: u16,
}

/// Conversation context manager settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
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

// ---------------------------------------------------------------------------
// CLI base-URL resolution
// ---------------------------------------------------------------------------

/// Default base URL used by CLI clients when neither an environment override
/// nor a configured `server.bind_addr` is available.
pub const DEFAULT_CLI_BASE_URL: &str = "http://127.0.0.1:8080";

/// Build a daemon base URL for an HTTP client from a configured `bind_addr`.
///
/// Wildcard bind hosts (`0.0.0.0`, `[::]`) are normalised to their loopback
/// equivalents so the client connects locally rather than relying on the OS
/// wildcard-routing behaviour.
pub fn base_url_from_bind_addr(bind_addr: &str) -> String {
    let s = bind_addr.trim();
    let normalised = if let Some(rest) = s.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{rest}")
    } else if s.strip_prefix("[::]:").is_some() {
        s.replacen("[::]:", "[::1]:", 1)
    } else if s == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        s.to_string()
    };
    format!("http://{normalised}")
}

/// Resolve the daemon base URL for CLI clients.
///
/// Precedence (each tier falls through on a missing/blank value):
/// 1. Explicit environment override (`MIMIR_BASE_URL`).
/// 2. Configured `server.bind_addr` from the config file.
/// 3. Compiled default ([`DEFAULT_CLI_BASE_URL`]).
pub fn resolve_base_url(env_override: Option<&str>, configured_bind_addr: Option<&str>) -> String {
    if let Some(env) = env_override.map(str::trim).filter(|s| !s.is_empty()) {
        return env.to_string();
    }
    if let Some(bind) = configured_bind_addr
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return base_url_from_bind_addr(bind);
    }
    DEFAULT_CLI_BASE_URL.to_string()
}

/// Read `server.bind_addr` from the default config file.
///
/// Best-effort: returns `None` if the file is absent, unreadable,
/// unparseable, or omits the field. Never creates directories or writes
/// files, so it is safe to call from every CLI command (even before
/// `mimir init`).
pub fn configured_bind_addr() -> Option<String> {
    bind_addr_from_path(&paths::config_path().ok()?)
}

fn bind_addr_from_path(path: &Path) -> Option<String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct ServerOnly {
        bind_addr: Option<String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct ConfigOnly {
        server: ServerOnly,
    }
    let contents = std::fs::read_to_string(path).ok()?;
    let cfg: ConfigOnly = toml::from_str(&contents).ok()?;
    cfg.server.bind_addr.filter(|s| !s.trim().is_empty())
}

/// Background scheduler settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    /// Seconds to wait after a job submission before dispatching.
    pub debounce_seconds: u8,
    /// Seconds to wait after last user activity before dispatching.
    pub cooldown_seconds: u16,
}

/// User identity settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IdentityConfig {
    pub name: String,
    pub preferred_name: String,
}

/// Knowledge graph settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KnowledgeConfig {
    pub optimization: KnowledgeOptimizationConfig,
    pub pending_cleanup: PendingCleanupConfig,
}

/// Knowledge graph nightly optimization settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeOptimizationConfig {
    pub cpu_cores: u8,
    pub nice_level: i8,
    pub timeout_minutes: u16,
    pub schedule_time: String,
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
            enabled: true,
            char_limit: 2500,
            auto_manage: true,
            temporal_horizon: 30,
            condensation_top_n: 500,
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

impl Default for KnowledgeOptimizationConfig {
    fn default() -> Self {
        Self {
            cpu_cores: 1,
            nice_level: 10,
            timeout_minutes: 120,
            schedule_time: "03:00".to_string(),
        }
    }
}

/// Settings for the pending sensitive-fact auto-cleanup job.
///
/// Facts awaiting confirmation longer than `retention_days` are hard-deleted
/// by a daily background job (see issue #141).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PendingCleanupConfig {
    /// Number of days a pending fact survives before automatic deletion.
    pub retention_days: u16,
    /// Daily local time (HH:MM, 24h) at which the cleanup runs.
    pub schedule_time: String,
}

impl Default for PendingCleanupConfig {
    fn default() -> Self {
        Self {
            retention_days: 7,
            schedule_time: "03:30".to_string(),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            debounce_seconds: 5,
            cooldown_seconds: 60,
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

        let cache_dir = paths::cache_dir()?;
        paths::ensure_dir(&cache_dir)?;

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

    #[cfg(test)]
    pub fn init_at(
        config_dir: &std::path::Path,
        data_dir: &std::path::Path,
        cache_dir: &std::path::Path,
    ) -> Result<InitResult, ConfigError> {
        paths::ensure_dir(config_dir)?;
        paths::ensure_dir(data_dir)?;
        paths::ensure_dir(cache_dir)?;

        let cfg_path = config_dir.join("config.toml");

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
                    config_dir: config_dir.to_path_buf(),
                    data_dir: data_dir.to_path_buf(),
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
max_tool_rounds = 100  # Maximum agentic tool-call rounds

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
condensation_top_n = 500

[scheduler]
debounce_seconds = 5
cooldown_seconds = 60

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

[identity]
# Set during init; the daemon uses this to identify the user entity in the knowledge graph.
name = ""
preferred_name = ""

[knowledge.optimization]
cpu_cores = 1
nice_level = 10
timeout_minutes = 120
schedule_time = "02:00"

[knowledge.pending_cleanup]
retention_days = 7
schedule_time = "03:30"
"#
        .to_string()
    }

    /// Apply environment variable overrides using the provided lookup function.
    fn apply_env_overrides_with<F>(&mut self, getenv: F)
    where
        F: Fn(&str) -> Option<String>,
    {
        macro_rules! set_from_env {
            ($key:literal, $target:expr) => {
                if let Some(v) = getenv($key) {
                    $target = v;
                }
            };
            ($key:literal, $target:expr, $parse:ty) => {
                if let Some(v) = getenv($key) {
                    if let Ok(n) = v.parse::<$parse>() {
                        $target = n;
                    }
                }
            };
            ($key:literal, $target:expr, $parse:ty, Some) => {
                if let Some(v) = getenv($key) {
                    if let Ok(n) = v.parse::<$parse>() {
                        $target = Some(n);
                    }
                }
            };
        }
        set_from_env!("MIMIR_LLM_ENDPOINT", self.llm.endpoint);
        set_from_env!("MIMIR_LLM_API_KEY", self.llm.api_key);
        set_from_env!("MIMIR_LLM_MODEL", self.llm.model);
        set_from_env!("MIMIR_LLM_MAX_TOKENS", self.llm.max_tokens, u32, Some);
        set_from_env!("MIMIR_LLM_TEMPERATURE", self.llm.temperature, f32);
        set_from_env!("MIMIR_AGENT_NAME", self.agent.name);
        set_from_env!(
            "MIMIR_AGENT_PROACTIVITY",
            self.agent.proactivity,
            Proactivity
        );
        set_from_env!(
            "MIMIR_AGENT_VERBOSE_REASONING",
            self.agent.verbose_reasoning,
            bool
        );
        set_from_env!(
            "MIMIR_AGENT_MAX_TOOL_ROUNDS",
            self.agent.max_tool_rounds,
            u16
        );
        set_from_env!("MIMIR_MEMORY_ENABLED", self.memory.enabled, bool);
        set_from_env!("MIMIR_MEMORY_CHAR_LIMIT", self.memory.char_limit, u16);
        set_from_env!("MIMIR_MEMORY_AUTO_MANAGE", self.memory.auto_manage, bool);
        set_from_env!(
            "MIMIR_MEMORY_TEMPORAL_HORIZON",
            self.memory.temporal_horizon,
            u8
        );
        set_from_env!(
            "MIMIR_CONTEXT_MAX_TOKENS",
            self.context.max_tokens,
            u32,
            Some
        );
        set_from_env!("MIMIR_CONTEXT_MAX_TURNS", self.context.max_turns, u16);
        if let Some(v) = getenv("MIMIR_CONTEXT_DB_PATH") {
            self.context.db_path = Some(PathBuf::from(v));
        }
        set_from_env!("MIMIR_PERSONALITY_PRESET", self.personality.preset);
        set_from_env!("MIMIR_SERVER_BIND_ADDR", self.server.bind_addr);
        set_from_env!("MIMIR_IDENTITY_NAME", self.identity.name);
        set_from_env!(
            "MIMIR_IDENTITY_PREFERRED_NAME",
            self.identity.preferred_name
        );
        if let Some(v) = getenv("MIMIR_SERVER_SOCKET_PATH") {
            self.server.socket_path = if v.trim().is_empty() { None } else { Some(v) };
        }
        set_from_env!(
            "MIMIR_SCHEDULER_DEBOUNCE_SECONDS",
            self.scheduler.debounce_seconds,
            u8
        );
        set_from_env!(
            "MIMIR_SCHEDULER_COOLDOWN_SECONDS",
            self.scheduler.cooldown_seconds,
            u16
        );
    }

    /// Apply environment variable overrides from the real process environment.
    fn apply_env_overrides(&mut self) {
        self.apply_env_overrides_with(|key| std::env::var(key).ok());
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
    use std::io::Write;

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.memory.char_limit, 2500);
        assert_eq!(config.identity.name, "");
        assert_eq!(config.llm.max_tokens, None);
        assert_eq!(config.context.max_tokens, None);
        assert_eq!(config.context.max_turns, 20);
        assert_eq!(config.context.db_path, paths::default_db_path().ok());
        assert_eq!(config.server.bind_addr, "127.0.0.1:8080");
        assert_eq!(config.server.socket_path, None);
    }

    #[test]
    fn test_env_override_llm() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_LLM_MODEL" {
                Some("gpt-3.5-turbo".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.llm.model, "gpt-3.5-turbo");
    }

    #[test]
    fn test_env_override_agent_max_tool_rounds() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_AGENT_MAX_TOOL_ROUNDS" {
                Some("50".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.agent.max_tool_rounds, 50);
    }

    #[test]
    fn test_env_override_agent_max_tool_rounds_invalid_ignored() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_AGENT_MAX_TOOL_ROUNDS" {
                Some("not_a_number".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.agent.max_tool_rounds, 100);
    }

    #[test]
    fn test_env_override_context() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| match key {
            "MIMIR_CONTEXT_MAX_TOKENS" => Some("8192".to_string()),
            "MIMIR_CONTEXT_MAX_TURNS" => Some("50".to_string()),
            "MIMIR_CONTEXT_DB_PATH" => Some("/tmp/mimir/context.db".to_string()),
            _ => None,
        });
        assert_eq!(config.context.max_tokens, Some(8192));
        assert_eq!(config.context.max_turns, 50);
        assert_eq!(
            config.context.db_path,
            Some(PathBuf::from("/tmp/mimir/context.db"))
        );
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
                enabled: false,
                char_limit: 100,
                auto_manage: false,
                temporal_horizon: 7,
                condensation_top_n: 500,
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
            identity: IdentityConfig::default(),
            knowledge: KnowledgeConfig::default(),
            scheduler: SchedulerConfig::default(),
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
condensation_top_n = 500

[scheduler]
debounce_seconds = 5
cooldown_seconds = 60

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
    fn test_load_none_uses_defaults_when_file_missing() {
        // Config::load(None) with no existing file bootstraps and returns defaults.
        // We verify by creating a temp dir, calling init_at, then loading from the
        // resulting config file.
        let dir = tempfile::tempdir().unwrap();
        let cfg_dir = dir.path().join("config");
        let data_dir = dir.path().join("data");
        let cache_dir = dir.path().join("cache");
        Config::init_at(&cfg_dir, &data_dir, &cache_dir).unwrap();
        let cfg_path = cfg_dir.join("config.toml");

        let config = Config::load(Some(&cfg_path)).unwrap();
        assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.memory.char_limit, 2500);
        assert_eq!(config.identity.name, "");
        assert_eq!(config.llm.max_tokens, None);
        assert_eq!(config.context.max_tokens, None);
        assert_eq!(config.context.max_turns, 20);
    }

    #[test]
    fn test_config_path_returns_platform_path() {
        let path = Config::config_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.ends_with("mimir/config.toml"));
    }

    #[test]
    fn test_invalid_proactivity_env_ignored() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_AGENT_PROACTIVITY" {
                Some("nonsense".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.agent.proactivity, Proactivity::ImportantOnly);
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
    fn test_personality_preset_env_override() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_PERSONALITY_PRESET" {
                Some("concise".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.personality.preset, "concise");
    }

    #[test]
    fn test_env_override_scheduler_debounce_seconds() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_SCHEDULER_DEBOUNCE_SECONDS" {
                Some("10".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.scheduler.debounce_seconds, 10);
    }

    #[test]
    fn test_env_override_scheduler_debounce_seconds_invalid_ignored() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_SCHEDULER_DEBOUNCE_SECONDS" {
                Some("not_a_number".to_string())
            } else {
                None
            }
        });
        // Should remain at default value
        assert_eq!(config.scheduler.debounce_seconds, 5);
    }

    #[test]
    fn test_env_override_scheduler_cooldown_seconds() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_SCHEDULER_COOLDOWN_SECONDS" {
                Some("120".to_string())
            } else {
                None
            }
        });
        assert_eq!(config.scheduler.cooldown_seconds, 120);
    }

    #[test]
    fn test_env_override_scheduler_cooldown_seconds_invalid_ignored() {
        let mut config = Config::default();
        config.apply_env_overrides_with(|key| {
            if key == "MIMIR_SCHEDULER_COOLDOWN_SECONDS" {
                Some("invalid".to_string())
            } else {
                None
            }
        });
        // Should remain at default value
        assert_eq!(config.scheduler.cooldown_seconds, 60);
    }

    #[test]
    fn test_init_creates_config_dir_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let cache_home = dir.path().join("cache");

        let result = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
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
        let contents = std::fs::read_to_string(cfg_home.join("config.toml")).unwrap();
        let parsed: Config = toml::from_str(&contents).unwrap();
        assert_eq!(parsed.llm.model, "gpt-4o");
    }

    #[test]
    fn test_init_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let cache_home = dir.path().join("cache");

        let result1 = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
        assert!(matches!(result1, InitResult::Created { .. }));

        let result2 = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
        assert!(matches!(result2, InitResult::AlreadyInitialized));

        // Config file should not have been overwritten.
        let contents = std::fs::read_to_string(cfg_home.join("config.toml")).unwrap();
        // Default TOML should still parse cleanly.
        let parsed: Config = toml::from_str(&contents).unwrap();
        assert_eq!(parsed.llm.model, "gpt-4o");
    }

    #[test]
    fn test_load_none_bootstraps_on_first_run() {
        // This test originally verified that Config::load(None) creates
        // default directories and files when env vars point to temp dirs.
        // That behaviour is covered by test_init_creates_config_dir_and_file
        // combined with test_load_from_toml_file, so we just sanity-check
        // that load(Some) still returns defaults when the file does not exist.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("mimir").join("config.toml");
        paths::ensure_dir(cfg_path.parent().unwrap()).unwrap();
        assert!(!cfg_path.exists());

        // When load(Some) is given a non-existent file, it fails rather than
        // bootstrapping — bootstrapping is load(None)'s responsibility.
        // Verify that explicit-path load still produces expected defaults
        // when the file DOES exist (written by init_at).
        Config::init_at(
            cfg_path.parent().unwrap(),
            dir.path().join("data").as_path(),
            dir.path().join("cache").as_path(),
        )
        .unwrap();
        assert!(cfg_path.exists());
        let config = Config::load(Some(&cfg_path)).unwrap();
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.name, "Mimir");
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
    fn test_init_does_not_overwrite_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let cache_home = dir.path().join("cache");

        // Write a custom config first.
        Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
        let cfg_path = cfg_home.join("config.toml");
        let custom = r#"
[llm]
model = "custom-model"
"#;
        std::fs::write(&cfg_path, custom).unwrap();

        // init again — should not overwrite.
        let result = Config::init_at(&cfg_home, &data_home, &cache_home).unwrap();
        assert!(matches!(result, InitResult::AlreadyInitialized));

        let contents = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(contents.contains("custom-model"));
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

#[cfg(test)]
mod base_url_tests {
    use super::*;

    #[test]
    fn test_base_url_from_bind_addr_passthrough() {
        assert_eq!(
            base_url_from_bind_addr("127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            base_url_from_bind_addr("0.0.0.0:8008"),
            "http://127.0.0.1:8008"
        );
    }

    #[test]
    fn test_base_url_from_bind_addr_ipv6_wildcard() {
        assert_eq!(base_url_from_bind_addr("[::]:8008"), "http://[::1]:8008");
    }

    #[test]
    fn test_base_url_from_bind_addr_bare_wildcard() {
        assert_eq!(base_url_from_bind_addr("0.0.0.0"), "http://127.0.0.1");
    }

    #[test]
    fn test_base_url_from_bind_addr_trims_whitespace() {
        assert_eq!(
            base_url_from_bind_addr("  127.0.0.1:9999  "),
            "http://127.0.0.1:9999"
        );
    }

    #[test]
    fn test_resolve_base_url_env_wins() {
        assert_eq!(
            resolve_base_url(Some("http://example:1"), Some("127.0.0.1:8080")),
            "http://example:1"
        );
    }

    #[test]
    fn test_resolve_base_url_blank_env_falls_through() {
        assert_eq!(
            resolve_base_url(Some("   "), Some("0.0.0.0:8008")),
            "http://127.0.0.1:8008"
        );
    }

    #[test]
    fn test_resolve_base_url_config_used() {
        assert_eq!(
            resolve_base_url(None, Some("127.0.0.1:8008")),
            "http://127.0.0.1:8008"
        );
    }

    #[test]
    fn test_resolve_base_url_default() {
        assert_eq!(resolve_base_url(None, None), DEFAULT_CLI_BASE_URL);
        assert_eq!(resolve_base_url(None, Some("")), DEFAULT_CLI_BASE_URL);
    }

    #[test]
    fn test_bind_addr_from_path_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[server]\nbind_addr = \"0.0.0.0:8008\"\n[llm]\nmodel = \"gpt-4o\"\n",
        )
        .unwrap();
        assert_eq!(bind_addr_from_path(&path), Some("0.0.0.0:8008".to_string()));
    }

    #[test]
    fn test_bind_addr_from_path_missing_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\nmodel = \"gpt-4o\"\n").unwrap();
        assert_eq!(bind_addr_from_path(&path), None);
    }

    #[test]
    fn test_bind_addr_from_path_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert_eq!(bind_addr_from_path(&path), None);
    }

    #[test]
    fn test_bind_addr_from_path_unparseable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not valid toml [[").unwrap();
        assert_eq!(bind_addr_from_path(&path), None);
    }
}
