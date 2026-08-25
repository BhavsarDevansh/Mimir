//! Credential resolution and OAuth token refresh.

use crate::connector::ConnectorError;
use crate::email::config::EmailAuthMethod;
use crate::email::connector::EmailConnector;
use crate::email::imap::ImapAuth;
use crate::oauth;
use crate::secrets::{AuthMethodDiscriminant, SecretBundle, mismatch_error};

/// Load the secret bundle and turn it into live [`ImapAuth`] credentials,
/// refreshing an expired OAuth access token (persisting the new bundle).
/// Returns the auth and whether a refresh happened.
impl EmailConnector {
    pub(super) async fn resolve_credentials(
        &self,
    ) -> Result<(ImapAuth, Option<SecretBundle>), ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?
            .ok_or(ConnectorError::NotAuthenticated)?;
        self.resolve_auth(&bundle, false).await
    }

    /// Turn a [`SecretBundle`] into [`ImapAuth`], refreshing an expired OAuth
    /// access token when needed. Returns the auth and the refreshed bundle (if
    /// a refresh happened) for the caller to persist.
    pub(crate) async fn resolve_auth(
        &self,
        bundle: &SecretBundle,
        force_refresh: bool,
    ) -> Result<(ImapAuth, Option<SecretBundle>), ConnectorError> {
        match (&self.config.auth, bundle) {
            (EmailAuthMethod::AppPassword { username }, SecretBundle::AppPassword { password }) => {
                Ok((
                    ImapAuth::Login {
                        username: username.clone(),
                        password: password.clone(),
                    },
                    None,
                ))
            }
            (
                EmailAuthMethod::OAuth {
                    username,
                    token_endpoint,
                    client_id,
                    client_secret,
                    scopes,
                    ..
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
                let (access_token, refreshed) = oauth::resolve_access_token(
                    http,
                    token_endpoint,
                    client_id,
                    client_secret.as_deref(),
                    scopes.as_deref(),
                    bundle,
                    force_refresh,
                )
                .await?;
                let auth = ImapAuth::Xoauth2 {
                    username: username.clone(),
                    access_token,
                };
                Ok((auth, refreshed))
            }
            _ => Err(mismatch_error(self.config.auth.discriminant())),
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
