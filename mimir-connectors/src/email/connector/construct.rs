//! `EmailConnector` construction from configuration.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tokio::sync::Mutex;

use crate::connector::ConnectorError;
use crate::email::config::{DEFAULT_DISPLAY_NAME, DEFAULT_SLUG, EmailConfigDto, parse_cursor};
use crate::email::connector::EmailConnector;
use crate::oauth::OAuthHttpClient;
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
        Self::from_config_with_deps(config, secret_store, None, cursor, None)
    }

    /// Build a connector with optional injected dependencies: the canonical
    /// user identity and a shared LLM backend (tests inject a mock; the
    /// daemon passes the live backend through the factory).
    pub fn from_config_with_deps(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
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
        let oauth_http = OAuthHttpClient::new()?;
        Ok(Self {
            slug,
            display_name: dto
                .display_name
                .clone()
                .unwrap_or_else(|| DEFAULT_DISPLAY_NAME.to_string()),
            config: dto,
            secret_store,
            oauth_http,
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
