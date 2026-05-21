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
}

/// LLM provider settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u16,
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
            let os = parent.as_os_str();
            if !os.is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    /// Return the canonical platform config file path for Mimir.
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("mimir").join("config.toml"))
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("MIMIR_LLM_API_KEY") {
            self.llm.api_key = val;
        }
        if let Ok(val) = std::env::var("MIMIR_LLM_ENDPOINT") {
            self.llm.endpoint = val;
        }
        if let Ok(val) = std::env::var("MIMIR_LLM_MODEL") {
            self.llm.model = val;
        }
        if let Ok(val) = std::env::var("MIMIR_LLM_MAX_TOKENS")
            && let Ok(parsed) = val.parse::<u16>()
        {
            self.llm.max_tokens = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_LLM_TEMPERATURE")
            && let Ok(parsed) = val.parse::<f32>()
        {
            self.llm.temperature = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_AGENT_NAME") {
            self.agent.name = val;
        }
        if let Ok(val) = std::env::var("MIMIR_AGENT_PROACTIVITY")
            && let Ok(parsed) = Proactivity::from_str(&val)
        {
            self.agent.proactivity = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_AGENT_VERBOSE_REASONING")
            && let Ok(parsed) = val.parse::<bool>()
        {
            self.agent.verbose_reasoning = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_MEMORY_ENABLED")
            && let Ok(parsed) = val.parse::<bool>()
        {
            self.memory.enabled = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_MEMORY_CHAR_LIMIT")
            && let Ok(parsed) = val.parse::<u16>()
        {
            self.memory.char_limit = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_MEMORY_AUTO_MANAGE")
            && let Ok(parsed) = val.parse::<bool>()
        {
            self.memory.auto_manage = parsed;
        }
        if let Ok(val) = std::env::var("MIMIR_MEMORY_TEMPORAL_HORIZON")
            && let Ok(parsed) = val.parse::<u8>()
        {
            self.memory.temporal_horizon = parsed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_default_values() {
        let config = Config::default();
        assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.llm.api_key, "");
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.llm.max_tokens, 4096);
        assert!((config.llm.temperature - 0.2).abs() < f32::EPSILON);
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.agent.proactivity, Proactivity::ImportantOnly);
        assert!(!config.agent.verbose_reasoning);
        assert!(config.memory.enabled);
        assert_eq!(config.memory.char_limit, 2500);
        assert!(config.memory.auto_manage);
        assert_eq!(config.memory.temporal_horizon, 30);
    }

    #[test]
    fn test_load_from_toml_file() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        let toml = r#"
[llm]
model = "gpt-3.5-turbo"
max_tokens = 1024

[agent]
name = "TestAgent"
proactivity = "always"

[memory]
char_limit = 500
"#;
        tmpfile.write_all(toml.as_bytes()).unwrap();
        let config = Config::load(Some(tmpfile.path())).unwrap();
        assert_eq!(config.llm.model, "gpt-3.5-turbo");
        assert_eq!(config.llm.max_tokens, 1024);
        assert_eq!(config.agent.name, "TestAgent");
        assert_eq!(config.agent.proactivity, Proactivity::Always);
        assert_eq!(config.memory.char_limit, 500);
        // Defaults for unspecified fields should be preserved.
        assert_eq!(config.llm.endpoint, "https://api.openai.com/v1");
        assert!(config.memory.enabled);
    }

    #[test]
    fn test_env_override_api_key() {
        unsafe { std::env::set_var("MIMIR_LLM_API_KEY", "sk-test-key") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.llm.api_key, "sk-test-key");
        unsafe { std::env::remove_var("MIMIR_LLM_API_KEY") };
    }

    #[test]
    fn test_env_override_proactivity() {
        unsafe { std::env::set_var("MIMIR_AGENT_PROACTIVITY", "never") };
        let config = Config::load(None).unwrap();
        assert_eq!(config.agent.proactivity, Proactivity::Never);
        unsafe { std::env::remove_var("MIMIR_AGENT_PROACTIVITY") };
    }

    #[test]
    fn test_invalid_proactivity_fails() {
        let result = Proactivity::from_str("sometimes");
        assert!(result.is_err());
        if let Err(ConfigError::InvalidProactivity(msg)) = result {
            assert_eq!(msg, "sometimes");
        } else {
            panic!("Expected InvalidProactivity error");
        }
    }

    #[test]
    fn test_save_and_reload() {
        let mut config = Config::default();
        config.agent.name = "RoundTrip".to_string();
        config.llm.model = "gpt-4-turbo".to_string();

        let tmpfile = tempfile::NamedTempFile::new().unwrap();
        config.save(tmpfile.path()).unwrap();

        let loaded = Config::load(Some(tmpfile.path())).unwrap();
        assert_eq!(config, loaded);
    }

    #[test]
    fn test_missing_file_uses_defaults() {
        // When load(None) is called and no file exists at the platform config
        // path, the compiled defaults are returned.
        let config = Config::load(None).unwrap();
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.agent.proactivity, Proactivity::ImportantOnly);
    }

    #[test]
    fn test_missing_explicit_file_errors() {
        let bogus = Path::new("/tmp/non_existent_mimir_config_12345.toml");
        let result = Config::load(Some(bogus));
        assert!(result.is_err());
    }
}
