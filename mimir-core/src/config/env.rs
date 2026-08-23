//! `MIMIR_*` environment-variable overrides.
//!
//! Overrides are applied after file/default loading with the documented
//! precedence. Parsing failures are silently ignored so a malformed variable
//! never aborts startup; invalid values fall back to the file value.

use std::path::PathBuf;

use crate::config::types::{Config, Proactivity, SecretsBackend};

impl Config {
    /// Apply environment variable overrides using the provided lookup function.
    pub(super) fn apply_env_overrides_with<F>(&mut self, getenv: F)
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
        set_from_env!(
            "MIMIR_AGENT_REMEMBER_DEBOUNCE_SECONDS",
            self.agent.remember_debounce_seconds,
            u8
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
        if let Some(v) = getenv("MIMIR_KNOWLEDGE_DB_PATH") {
            self.knowledge.db_path = Some(PathBuf::from(v));
        }
        if let Some(v) = getenv("MIMIR_JOBS_DB_PATH") {
            self.scheduler.db_path = Some(PathBuf::from(v));
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
        set_from_env!(
            "MIMIR_SECRETS_BACKEND",
            self.secrets.backend,
            SecretsBackend
        );
        set_from_env!("MIMIR_GEOCODER_ENABLED", self.geocoder.enabled, bool);
        set_from_env!("MIMIR_GEOCODER_ENDPOINT", self.geocoder.endpoint);
        if let Some(v) = getenv("MIMIR_GEOCODER_CONTACT_EMAIL") {
            self.geocoder.contact_email = if v.trim().is_empty() {
                None
            } else {
                Some(v.trim().to_string())
            };
        }
        if let Some(v) = getenv("MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES") {
            let times: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !times.is_empty() {
                self.knowledge.events.schedule_times = times;
            }
        }
        set_from_env!(
            "MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS",
            self.knowledge.events.horizon_days,
            u16
        );
    }

    /// Apply environment variable overrides from the real process environment.
    pub(crate) fn apply_env_overrides(&mut self) {
        self.apply_env_overrides_with(|key| std::env::var(key).ok());
    }
}
