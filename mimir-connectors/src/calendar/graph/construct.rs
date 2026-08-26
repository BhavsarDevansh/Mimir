//! `GraphCalendarConnector` construction from configuration and credential
//! resolution into a live Graph client.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::calendar::CalendarAuthMethod;
use crate::calendar::graph::client::{GRAPH_BASE_URL, GraphClient};
use crate::calendar::graph::{
    DEFAULT_DISPLAY_NAME, DEFAULT_SLUG, GraphCalendarConfigDto, GraphCalendarConnector,
};
use crate::connector::{ConnectorError, normalize_user_identity};
use crate::oauth::OAuthHttpClient;
use crate::secrets::SecretStore;

impl GraphCalendarConnector {
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
    /// client pointed at a mock server; production passes `None` for a
    /// default 30 s-timeout client).
    pub fn from_config_with_http(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
        http: Option<reqwest::Client>,
    ) -> Result<Self, ConnectorError> {
        // Recover the supervisor-injected slug before parsing the DTO: serde
        // ignores unknown fields (the DTO has no `deny_unknown_fields`), so
        // the injected `__slug` / `__cursor` keys pass through harmlessly.
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());
        let dto: GraphCalendarConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid calendar config: {e}")))?;
        // Microsoft Graph is OAuth-only: an app-password config cannot
        // authenticate against the Graph API, so reject it at construction
        // with a clear message instead of failing at the first sync.
        if !matches!(dto.auth, CalendarAuthMethod::OAuth { .. }) {
            return Err(ConnectorError::Config(
                "Microsoft Graph calendar requires OAuth auth (kind=oauth); app passwords are not supported by the Graph API".into(),
            ));
        }
        let http = match http {
            Some(c) => c,
            None => reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| ConnectorError::Config(format!("HTTP client build failed: {e}")))?,
        };
        let oauth_http = Some(OAuthHttpClient::new()?);
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
            delta_link: Mutex::new(cursor.filter(|c| !c.is_empty())),
            buffer: Mutex::new(Vec::new()),
            tombstones: Mutex::new(Vec::new()),
        })
    }

    /// The configured Graph service root (defaults to
    /// [`GRAPH_BASE_URL`]).
    pub(super) fn base_url(&self) -> &str {
        self.config.base_url.as_deref().unwrap_or(GRAPH_BASE_URL)
    }

    /// Build a [`GraphClient`] from the current credentials.
    ///
    /// Loads the [`SecretBundle`](crate::secrets::SecretBundle) by slug and
    /// refreshes an expired access token first. Returns the client and the
    /// (possibly refreshed) bundle so the caller can persist it.
    pub(super) async fn client_from_credentials(
        &self,
    ) -> Result<(GraphClient, Option<crate::secrets::SecretBundle>), ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?
            .ok_or(ConnectorError::NotAuthenticated)?;
        let (token, refreshed) = self.resolve_auth(&bundle, false).await?;
        Ok((
            GraphClient::new(self.http.clone(), self.base_url().to_string(), token),
            refreshed,
        ))
    }
}
