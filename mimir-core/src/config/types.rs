use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use crate::paths;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    /// Debounce window (seconds) for the `remember.chat` hook: consecutive
    /// turns within the window replace the pending extraction with the
    /// accumulated transcript, so a burst of messages becomes one extraction
    /// (issue #386).
    pub remember_debounce_seconds: u8,
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

/// Background scheduler settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    /// Seconds to wait after a job submission before dispatching.
    pub debounce_seconds: u8,
    /// Seconds to wait after last user activity before dispatching.
    pub cooldown_seconds: u16,
    /// Optional override for the job-queue SQLite database path. When unset,
    /// the daemon resolves the default (`<data_dir>/jobs.db`) via
    /// [`paths::jobs_db_path`]. Mirrors `context.db_path` so tests and
    /// multi-instance setups can isolate the jobs DB from the shared data
    /// directory (issue #233).
    pub db_path: Option<PathBuf>,
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
    pub events: EventsConfig,
    /// Optional override for the knowledge-graph SQLite database path.
    /// When unset, the daemon resolves the default (`<data_dir>/knowledge.db`)
    /// via [`paths::knowledge_db_path`]. Mirrors `context.db_path` so tests
    /// and multi-instance setups can isolate the knowledge DB from the
    /// shared data directory (issue #233).
    pub db_path: Option<PathBuf>,
}

/// Knowledge graph nightly optimization settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeOptimizationConfig {
    pub cpu_cores: u8,
    pub nice_level: i8,
    pub timeout_minutes: u16,
    pub schedule_time: String,
    /// Best-effort memory cap (MiB) for the whole process while the
    /// optimization job runs (Linux cgroup v2 only; skipped when unavailable).
    pub memory_limit_mb: Option<u32>,
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
            remember_debounce_seconds: 10,
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
            memory_limit_mb: None,
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

/// Settings for the events & reminders upcoming-scan job (issue #74).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventsConfig {
    /// Daily local times (HH:MM, 24h) at which the scan runs.
    pub schedule_times: Vec<String>,
    /// How many days into the future the derive pass looks for upcoming facts.
    pub horizon_days: u16,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            schedule_times: vec!["06:00".to_string(), "18:00".to_string()],
            horizon_days: 30,
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            debounce_seconds: 5,
            cooldown_seconds: 60,
            db_path: None,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
