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
    pub geocoder: GeocoderConfig,
    pub secrets: SecretsConfig,
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
    pub compaction: ContextCompactionConfig,
}

/// Background session-compaction settings (issue #279).
///
/// When enabled, the `session.compaction` hook summarises the oldest
/// complete turns beyond `max_turns` via the LLM, stores the summary on the
/// session, and deletes the summarised messages — so trimming never silently
/// discards context. Keep `max_turns` below [`ContextConfig::max_turns`] so
/// compaction summarises turns before the synchronous trim removes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextCompactionConfig {
    /// Master switch for the background compaction hook.
    pub enabled: bool,
    /// Number of most recent complete turns to keep; older complete turns
    /// are summarised and removed.
    pub max_turns: u16,
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
    /// When unset, the default `<data_dir>/mimir.sock` is used on Unix
    /// platforms (see [`crate::config::effective_socket_path`]); Unix sockets
    /// are unavailable on Windows.
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
    /// Optional override for the Obsidian export directory (issue #62).
    ///
    /// When unset, `mimir kb export` writes to `~/AgentKnowledge`. Mirrors
    /// the `MIMIR_KNOWLEDGE_EXPORT_DIR` environment variable.
    pub export_dir: Option<PathBuf>,
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

/// Geocoding settings (Phase 3 S1 / issue #227).
///
/// Controls the shared [`Geocoder`](crate::geocoder::Geocoder) injected into
/// the knowledge-graph entity-locations write path and the Photos connector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeocoderConfig {
    /// Master switch. When `false`, no geocoder is constructed at startup:
    /// location facts persist with whatever coordinates/address the producer
    /// supplied and the missing half is never filled in.
    pub enabled: bool,
    /// Base endpoint of the OSM Nominatim instance (no trailing slash).
    /// Defaults to the public instance; point at a self-hosted Nominatim for
    /// heavy use.
    pub endpoint: String,
    /// Optional contact email appended to the `User-Agent` sent to the
    /// instance (recommended for the public instance).
    pub contact_email: Option<String>,
}

impl Default for GeocoderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpoint: crate::geocoder::DEFAULT_NOMINATIM_ENDPOINT.to_string(),
            contact_email: None,
        }
    }
}

/// Connector credential storage settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SecretsConfig {
    /// Which secret-store backend the daemon uses for connector secrets.
    ///
    /// `file` (the default) stores plaintext per-slug JSON files under
    /// `~/.local/share/mimir/secrets/` with `0600`/`0700` permissions;
    /// `keychain` stores bundles in the OS credential store (macOS Keychain,
    /// Linux/BSD Secret Service, Windows Credential Manager) and requires a
    /// build with the `secrets-keyring` cargo feature.
    pub backend: SecretsBackend,
}

/// Which credential store backs connector secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SecretsBackend {
    /// Per-connector JSON files with strict Unix permissions (V1 default).
    #[default]
    File,
    /// The OS credential store via the `keyring` crate (`secrets-keyring`
    /// cargo feature; macOS Keychain / Linux or BSD Secret Service / Windows
    /// Credential Manager).
    Keychain,
}

impl fmt::Display for SecretsBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretsBackend::File => write!(f, "file"),
            SecretsBackend::Keychain => write!(f, "keychain"),
        }
    }
}

impl FromStr for SecretsBackend {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" => Ok(SecretsBackend::File),
            "keychain" => Ok(SecretsBackend::Keychain),
            _ => Err(ConfigError::InvalidSecretsBackend(s.to_string())),
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

    /// The supplied secrets backend value is not recognised.
    #[error("Invalid secrets backend: '{0}'. Expected 'file' or 'keychain'.")]
    InvalidSecretsBackend(String),
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
            compaction: ContextCompactionConfig::default(),
        }
    }
}

impl Default for ContextCompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // Below the default `context.max_turns` (20) so compaction runs
            // ahead of the synchronous trim (issue #279).
            max_turns: 15,
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
        assert!(config.context.compaction.enabled);
        assert_eq!(config.context.compaction.max_turns, 15);
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
