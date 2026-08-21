//! First-run bootstrap: directory creation and default `config.toml` generation.

use crate::config::types::{Config, ConfigError, InitResult};
use crate::paths;

impl Config {
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

        // Generate the local API token (issue #281) so the daemon and CLI can
        // authenticate each other from first run. Best-effort: the daemon and
        // CLI also create it lazily, so a failure here is not fatal.
        if let Err(e) = crate::auth::load_or_create_api_token() {
            tracing::warn!("Failed to create API token during init: {e}");
        }

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

        if let Err(e) = crate::auth::load_or_create_api_token_at(&data_dir.join("api_token")) {
            tracing::warn!("Failed to create API token during init: {e}");
        }

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
    pub(super) fn default_config_toml() -> String {
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
max_tool_rounds = 100
remember_debounce_seconds = 10  # Debounce window for the remember.chat hook

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
condensation_top_n = 500

[scheduler]
debounce_seconds = 5
cooldown_seconds = 60
# db_path is resolved automatically; override only if needed.
# db_path = "${USER_DATA_DIR}/mimir/jobs.db"

[context]
max_turns = 20
# db_path is resolved automatically; override only if needed.
# db_path = "${USER_DATA_DIR}/mimir/context.db"
# max_tokens = 4096  # Optional: token budget for conversation history

[knowledge]
# db_path is resolved automatically; override only if needed.
# db_path = "${USER_DATA_DIR}/mimir/knowledge.db"

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
# memory_limit_mb = 2048  # Optional: best-effort cgroup v2 memory cap (MiB)

[knowledge.pending_cleanup]
retention_days = 7
schedule_time = "03:30"

[knowledge.events]
schedule_times = ["06:00", "18:00"]
horizon_days = 30
"#
        .to_string()
    }
}
