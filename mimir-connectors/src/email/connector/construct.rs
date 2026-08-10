//! `EmailConnector` construction from configuration.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::connector::ConnectorError;
use crate::email::config::{DEFAULT_DISPLAY_NAME, DEFAULT_SLUG, EmailConfigDto, parse_cursor};
use crate::email::connector::EmailConnector;
use crate::secrets::SecretStore;
use mimir_core::llm::LlmBackend;

/// Build a connector from its parsed configuration, a shared secret store
/// (optional), and the supervisor-injected cursor.
impl EmailConnector {
    pub fn from_config(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        cursor: Option<String>,
    ) -> Result<Self, ConnectorError> {
        Self::from_config_with_http(config, secret_store, None, cursor, None, None)
    }

    /// Build a connector, allowing an injected `http` client (tests inject a
    /// client pointed at a mock token endpoint; production passes `None` for a
    /// default 30 s-timeout client).
    pub fn from_config_with_http(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
        http: Option<reqwest::Client>,
        llm_backend: Option<Arc<dyn LlmBackend>>,
    ) -> Result<Self, ConnectorError> {
        // Recover the supervisor-injected slug before parsing the DTO: serde
        // ignores unknown fields (the DTO has no `deny_unknown_fields`), so
        // the injected `__slug` / `__cursor` keys pass through harmlessly.
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());
        let dto: EmailConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid email config: {e}")))?;
        let http = match http {
            Some(c) => c,
            None => reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| ConnectorError::Config(format!("HTTP client build failed: {e}")))?,
        };
        Ok(Self {
            slug,
            display_name: dto
                .display_name
                .clone()
                .unwrap_or_else(|| DEFAULT_DISPLAY_NAME.to_string()),
            config: dto,
            secret_store,
            http,
            last_uid: Mutex::new(cursor.as_deref().and_then(parse_cursor)),
            supports_idle: StdMutex::new(None),
            buffer: Mutex::new(Vec::new()),
            user_identity: user_identity
                .filter(|n| !n.trim().is_empty())
                .map(|n| n.trim().to_string()),
            llm_backend,
        })
    }
}
