//! Configuration subsystem: typed settings, file/environment loading,
//! and live reload.
//!
//! - `types` — the configuration structs, enums, and defaults.
//! - `base_url` — CLI base-URL resolution from config and environment.
//! - `load` — loading, saving, initialisation, and environment overrides.
//! - `reload` — runtime reload with sensitive-field guards.

mod base_url;
mod env;
mod init;
mod load;
mod reload;
mod socket;
#[cfg(test)]
mod tests;
mod types;

pub use base_url::{
    DEFAULT_CLI_BASE_URL, base_url_from_bind_addr, configured_bind_addr, resolve_base_url,
};
pub use reload::{ConfigReloadError, ReloadableConfig};
pub use socket::{configured_socket_path, effective_socket_path};
pub use types::{
    AgentConfig, Config, ConfigError, ContextConfig, EventsConfig, GeocoderConfig, IdentityConfig,
    InitResult, KnowledgeConfig, KnowledgeOptimizationConfig, LlmConfig, MemoryConfig,
    PendingCleanupConfig, PersonalityConfig, Proactivity, SchedulerConfig, SecretsBackend,
    SecretsConfig, ServerConfig,
};
