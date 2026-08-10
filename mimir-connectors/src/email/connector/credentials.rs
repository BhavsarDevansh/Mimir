//! Credential resolution and OAuth token refresh.

use chrono::Utc;

use crate::connector::ConnectorError;
use crate::email::config::EmailAuthMethod;
use crate::email::connector::EmailConnector;
use crate::email::imap::ImapAuth;
use crate::oauth;
use crate::secrets::SecretBundle;

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
        self.resolve_auth(&bundle).await
    }

    /// Turn a [`SecretBundle`] into [`ImapAuth`], refreshing an expired OAuth
    /// access token when needed. Returns the auth and the refreshed bundle (if
    /// a refresh happened) for the caller to persist.
    pub(crate) async fn resolve_auth(
        &self,
        bundle: &SecretBundle,
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
                EmailAuthMethod::OAuth { .. },
                SecretBundle::OAuth {
                    access_token,
                    refresh_token,
                    expires_at,
                },
            ) => {
                // Refresh if expired (or within 60 s of expiry). An unknown
                // expiry (`None`) does not force a refresh every cycle — the
                // token is reused and the server's 401 triggers re-auth next.
                let needs_refresh = expires_at
                    .map(|exp| exp <= Utc::now() + chrono::Duration::seconds(60))
                    .unwrap_or(false);
                if needs_refresh {
                    let refresh_token = refresh_token.clone().ok_or_else(|| {
                        ConnectorError::Authentication(
                            "OAuth access token expired and no refresh token is stored".into(),
                        )
                    })?;
                    let refreshed = self.refresh_oauth(&refresh_token).await?;
                    let token = refreshed.access_token.clone().ok_or_else(|| {
                        ConnectorError::Authentication(
                            "token endpoint returned no access_token".into(),
                        )
                    })?;
                    let bundle = refreshed.into_bundle(Some(refresh_token));
                    let auth = ImapAuth::Xoauth2 {
                        username: self.oauth_username().to_string(),
                        access_token: token,
                    };
                    Ok((auth, Some(bundle)))
                } else {
                    Ok((
                        ImapAuth::Xoauth2 {
                            username: self.oauth_username().to_string(),
                            access_token: access_token.clone(),
                        },
                        None,
                    ))
                }
            }
            _ => Err(ConnectorError::Authentication(format!(
                "auth method {} does not match stored secret kind",
                self.config.auth.discriminant()
            ))),
        }
    }

    /// The OAuth account username (panics if the configured auth is not OAuth
    /// — only called from within the `OAuth` arm of [`resolve_auth`]).
    fn oauth_username(&self) -> &str {
        match &self.config.auth {
            EmailAuthMethod::OAuth { username, .. } => username,
            _ => unreachable!("oauth_username called for a non-OAuth email connector"),
        }
    }

    /// Refresh an OAuth access token via the configured token endpoint,
    /// delegating to the shared [`crate::oauth::refresh_token`] helper (DRY
    /// with the Calendar connector).
    async fn refresh_oauth(
        &self,
        refresh_token: &str,
    ) -> Result<oauth::RefreshTokenResponse, ConnectorError> {
        let EmailAuthMethod::OAuth {
            token_endpoint,
            client_id,
            client_secret,
            scopes,
            ..
        } = &self.config.auth
        else {
            return Err(ConnectorError::Config(
                "refresh_oauth called for a non-OAuth connector".into(),
            ));
        };
        oauth::refresh_token(
            &self.http,
            token_endpoint,
            client_id,
            client_secret.as_deref(),
            scopes.as_deref(),
            refresh_token,
        )
        .await
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
