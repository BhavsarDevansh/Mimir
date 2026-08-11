//! Credential resolution and OAuth token refresh for the CalDAV connector.

use super::CalendarAuthMethod;
use crate::calendar::caldav::CalDavAuth;

use crate::calendar::CalendarConnector;
use crate::connector::ConnectorError;
use crate::oauth;
use crate::secrets::SecretBundle;

impl CalendarConnector {
    /// Turn a [`SecretBundle`] into a [`CalDavAuth`], refreshing an expired
    /// OAuth token when needed. Returns the auth and the refreshed bundle (if
    /// a refresh happened) for the caller to persist.
    pub(super) async fn resolve_auth(
        &self,
        bundle: &SecretBundle,
    ) -> Result<(CalDavAuth, Option<SecretBundle>), ConnectorError> {
        match (&self.config.auth, bundle) {
            (
                CalendarAuthMethod::AppPassword { username },
                SecretBundle::AppPassword { password },
            ) => Ok((
                CalDavAuth::Basic {
                    username: username.clone(),
                    password: password.clone(),
                },
                None,
            )),
            (
                CalendarAuthMethod::OAuth {
                    token_endpoint,
                    client_id,
                    client_secret,
                    scopes,
                },
                SecretBundle::OAuth { .. },
            ) => {
                // Resolve a live access token through the shared OAuth refresh
                // path (issue #240: `oauth2` 5.0.0 over the workspace reqwest
                // 0.13 client). Returns the token to use and, when a refresh
                // happened, the refreshed bundle for the caller to persist.
                let http = self.oauth_http.as_ref().ok_or_else(|| {
                    ConnectorError::Config(
                        "OAuth auth method configured without an OAuth HTTP client".into(),
                    )
                })?;
                let (token, refreshed) = oauth::resolve_access_token(
                    http,
                    token_endpoint,
                    client_id,
                    client_secret.as_deref(),
                    scopes.as_deref(),
                    bundle,
                )
                .await?;
                Ok((CalDavAuth::Bearer { token }, refreshed))
            }
            // Auth method / bundle kind mismatch — e.g. an app-password bundle
            // configured as OAuth, or vice versa.
            _ => Err(ConnectorError::Authentication(format!(
                "auth method {} does not match stored secret kind",
                self.config.auth.discriminant()
            ))),
        }
    }

    /// Persist a refreshed OAuth bundle back to the secret store.
    pub(super) async fn persist_refreshed(
        &self,
        bundle: &SecretBundle,
    ) -> Result<(), ConnectorError> {
        if let Some(store) = &self.secret_store {
            store.store(&self.slug, bundle).await.map_err(|e| {
                ConnectorError::Authentication(format!("secret persist failed: {e}"))
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
