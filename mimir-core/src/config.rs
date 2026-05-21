use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
}

/// LLM provider settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// Agent behaviour settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub name: String,
    pub proactivity: Proactivity,
    pub verbose_reasoning: bool,
}

/// Memory subsystem settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub char_limit: u16,
    pub auto_manage: bool,
    pub temporal_horizon: u8,
}

/// Conversation context manager settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub max_tokens: u32,
    pub max_turns: u16,
    pub db_path: Option<PathBuf>,
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

    /// The supplied proactivity value is not recognised.
    #[error("Invalid proactivity value: '{0}'. Expected 'never', 'important_only', or 'always'.")]
    InvalidProactivity(String),

    /// The platform configuration directory could not be determined.
    #[error("Could not determine config directory")]
    MissingConfigDir,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            max_tokens: 4096,
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
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            max_turns: 20,
            db_path: Some(PathBuf::from("~/.local/share/mimir/context.db")),
        }
    }
}

impl Config {
    /// Load configuration, applying the precedence:
    /// 1. Compiled defaults
    /// 2. TOML file (optional path or default location)
    /// 3. `MIMIR_*` environment variables
    ///
    /// If `path` is `Some`, the file must exist or an error is returned.
    /// If `path` is `None`, the default platform config path is tried; if the file
    /// does not exist the compiled defaults are used.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut config = Config::default();

        match path {
            Some(p) => {
                let contents = std::fs::read_to_string(p)?;
                config = toml::from_str(&contents)?;
            }
            None => {
                if let Some(config_path) = Self::config_path()
                    && config_path.exists()
                {
                    let contents = std::fs::read_to_string(&config_path)?;
                    config = toml::from_str(&contents)?;
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
        dirs::config_dir().map(|p| p.join("mimir").join("config.toml"))
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
            self.llm.max_tokens = n;
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
        if let Ok(v) = std::env::var("MIMIR_CONTEXT_MAX_TOKENS")
            && let Ok(n) = v.parse::<u32>()
        {
            self.context.max_tokens = n;
        }
        if let Ok(v) = std::env::var("MIMIR_CONTEXT_MAX_TURNS")
            && let Ok(n) = v.parse::<u16>()
        {
            self.context.max_turns = n;
        }
        if let Ok(v) = std::env::var("MIMIR_CONTEXT_DB_PATH") {
            self.context.db_path = Some(PathBuf::from(v));
        }
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
        assert_eq!(config.context.max_tokens, 4096);
        assert_eq!(config.context.max_turns, 20);
        assert_eq!(
            config.context.db_path,
            Some(PathBuf::from("~/.local/share/mimir/context.db"))
        );
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
        assert_eq!(config.context.max_tokens, 8192);
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
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let original = Config {
            llm: LlmConfig {
                endpoint: "http://localhost:8080".to_string(),
                api_key: "secret".to_string(),
                model: "test-model".to_string(),
                max_tokens: 100,
                temperature: 0.5,
            },
            agent: AgentConfig {
                name: "TestAgent".to_string(),
                proactivity: Proactivity::Always,
                verbose_reasoning: true,
            },
            memory: MemoryConfig {
                enabled: false,
                char_limit: 100,
                auto_manage: false,
                temporal_horizon: 7,
            },
            context: ContextConfig {
                max_tokens: 2048,
                max_turns: 10,
                db_path: Some(PathBuf::from("~/.local/share/mimir/context.db")),
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

[context]
max_tokens = 4096
max_turns = 20
db_path = "~/.local/share/mimir/context.db"
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.context.max_tokens, 4096);
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
        assert_eq!(config.context.max_tokens, 4096);
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
        assert_eq!(config.context.max_tokens, 4096); // default
    }
}
