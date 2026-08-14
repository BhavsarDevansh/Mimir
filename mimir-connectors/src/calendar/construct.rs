//! `CalendarConnector` construction from configuration and credential
//! resolution into a live CalDAV client.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::calendar::CalendarConnector;
use crate::calendar::caldav::CalDavClient;
use crate::connector::{ConnectorError, normalize_user_identity};
use crate::oauth::OAuthHttpClient;
use crate::secrets::{SecretBundle, SecretStore};

use super::{CalendarAuthMethod, CalendarConfigDto, DEFAULT_DISPLAY_NAME, DEFAULT_SLUG};

impl CalendarConnector {
    /// Build a connector from its parsed configuration, an optional secret
    /// store, and the supervisor-injected cursor.
    pub fn from_config(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        cursor: Option<String>,
    ) -> Result<Self, ConnectorError> {
        Self::from_config_with_http(config, secret_store, None, cursor, None)
    }

    /// Build a connector, allowing an injected `http` client (tests inject a
    /// client pointed at a mock server; production passes `None` for a default
    /// 30 s-timeout client).
    pub fn from_config_with_http(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
        http: Option<reqwest::Client>,
    ) -> Result<Self, ConnectorError> {
        // Recover the supervisor-injected slug before parsing the DTO: serde
        // ignores unknown fields (the DTO has no `deny_unknown_fields`), so the
        // injected `__slug` / `__cursor` keys pass through harmlessly.
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());
        let dto: CalendarConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid calendar config: {e}")))?;
        let http = match http {
            Some(c) => c,
            None => reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| ConnectorError::Config(format!("HTTP client build failed: {e}")))?,
        };
        // Only an OAuth-configured connector needs the hardened OAuth client;
        // an app-password connector must not allocate a second reqwest
        // connection pool or fail startup if the OAuth client build fails.
        let oauth_http = match &dto.auth {
            CalendarAuthMethod::OAuth { .. } => Some(OAuthHttpClient::new()?),
            CalendarAuthMethod::AppPassword { .. } => None,
        };
        Ok(Self {
            slug,
            display_name: dto
                .display_name
                .clone()
                .unwrap_or_else(|| DEFAULT_DISPLAY_NAME.to_string()),
            config: dto,
            user_identity: normalize_user_identity(user_identity),
            secret_store,
            http,
            oauth_http,
            sync_token: Mutex::new(cursor.filter(|c| !c.is_empty())),
            buffer: Mutex::new(Vec::new()),
        })
    }

    /// Build a [`CalDavClient`] from the current credentials.
    ///
    /// Loads the [`SecretBundle`] by slug; for OAuth, refreshes an expired
    /// access token first. Returns the client and the (possibly refreshed)
    /// bundle so the caller can persist it.
    pub(super) async fn client_from_credentials(
        &self,
    ) -> Result<(CalDavClient, Option<SecretBundle>), ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?
            .ok_or(ConnectorError::NotAuthenticated)?;
        let (auth, refreshed) = self.resolve_auth(&bundle).await?;
        Ok((CalDavClient::new(self.http.clone(), auth), refreshed))
    }
}
