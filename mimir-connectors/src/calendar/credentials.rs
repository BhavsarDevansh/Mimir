//! Credential resolution and OAuth token refresh for the CalDAV connector.

use chrono::Utc;

use super::CalendarAuthMethod;
use crate::calendar::caldav::CalDavAuth;

use crate::calendar::CalendarConnector;
use crate::connector::ConnectorError;
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
                CalendarAuthMethod::OAuth { .. },
                SecretBundle::OAuth {
                    access_token,
                    refresh_token,
                    expires_at,
                },
            ) => {
                // Refresh if expired (or within a 60 s skew of expiry). An
                // unknown expiry (`None`) does not force a refresh on every
                // cycle — that would triple the POSTs against the token
                // endpoint and invite rate limiting. The token is reused
                // as-is; if it is actually expired the server returns 401 and
                // the next cycle re-authenticates.
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
                    let auth = CalDavAuth::Bearer { token };
                    Ok((auth, Some(refreshed.into_bundle(Some(refresh_token)))))
                } else {
                    Ok((
                        CalDavAuth::Bearer {
                            token: access_token.clone(),
                        },
                        None,
                    ))
                }
            }
            // Auth method / bundle kind mismatch — e.g. an app-password bundle
            // configured as OAuth, or vice versa.
            _ => Err(ConnectorError::Authentication(format!(
                "auth method {} does not match stored secret kind",
                self.config.auth.discriminant()
            ))),
        }
    }

    /// Refresh an OAuth access token via the configured token endpoint.
    ///
    /// Delegates to the shared [`crate::oauth::refresh_token`] helper so the
    /// Calendar and Email connectors share one refresh implementation (DRY).
    /// The `oauth2` crate is avoided: it depends on reqwest 0.12, which would
    /// duplicate the workspace's reqwest 0.13 stack; a refresh is a single
    /// form-encoded HTTPS POST returning JSON. The interactive PKCE login
    /// that *obtains* the first token is A4 / #205.
    async fn refresh_oauth(
        &self,
        refresh_token: &str,
    ) -> Result<crate::oauth::RefreshTokenResponse, ConnectorError> {
        let CalendarAuthMethod::OAuth {
            token_endpoint,
            client_id,
            client_secret,
            scopes,
        } = &self.config.auth
        else {
            return Err(ConnectorError::Config(
                "refresh_oauth called for a non-OAuth connector".into(),
            ));
        };
        crate::oauth::refresh_token(
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
