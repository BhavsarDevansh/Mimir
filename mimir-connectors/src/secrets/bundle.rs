//! Credential bundle types: the per-connector secret payloads.
//!
//! [`SecretBundle`] is the single discriminated union stored under every
//! connector slug. Its `Debug` impl is hand-written (rather than derived) so
//! secret values are always redacted when a store, error, or log line is
//! formatted.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The credentials for a single connector instance.
///
/// Exactly one variant applies per connector, determined by its auth method:
/// OAuth 2.0 (Gmail, Google Calendar), an API token (Home Assistant, GitHub
/// PAT), or an app password (Fastmail, legacy IMAP).
///
/// Serialized with an internal `kind` tag so each file is human-inspectable:
/// `{"kind":"oauth","access_token":...}`, `{"kind":"api_token","token":...}`,
/// `{"kind":"app_password","password":...}`.
///
/// Struct variants are used (rather than newtype variants like `ApiToken(String)`)
/// because serde's internally-tagged representation requires map-typed variant
/// payloads; the named fields also make the on-disk JSON self-describing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SecretBundle {
    /// OAuth 2.0 access token with optional refresh token and expiry.
    ///
    /// `refresh_token` is `None` for grants that do not issue one (e.g.
    /// client-credentials); `expires_at` is `None` when the provider does not
    /// return an expiry.
    #[serde(rename = "oauth")]
    OAuth {
        /// Short-lived bearer token presented to the service.
        access_token: String,
        /// Long-lived token used to refresh `access_token`; may be absent.
        refresh_token: Option<String>,
        /// When `access_token` expires, or `None` if unknown.
        expires_at: Option<DateTime<Utc>>,
        /// OAuth client secret for confidential clients. Stored with the
        /// credential bundle (never in `config_json`); `None` for PKCE
        /// public clients and for bundles persisted before this field
        /// existed (serde `default` keeps those files loadable).
        #[serde(default)]
        client_secret: Option<String>,
    },
    /// A static API token presented as a bearer/`Authorization` header.
    ApiToken {
        /// The secret token string.
        token: String,
    },
    /// An app-specific password (e.g. Fastmail, legacy IMAP).
    ///
    /// Only the secret `password` lives in the bundle. The accompanying
    /// username is part of the connector instance's non-secret `config_json`
    /// (stored on the `connectors` row), not the credential store — so it is
    /// not duplicated here.
    AppPassword {
        /// The app-specific password.
        password: String,
    },
}

impl std::fmt::Debug for SecretBundle {
    /// Redacted `Debug`: variant discriminant and non-secret fields only.
    ///
    /// The secret values (`access_token`, `refresh_token`, `token`,
    /// `password`) are replaced with `"<redacted>"` so that
    /// `Debug`-formatting a [`SecretStore`](super::store::SecretStore) (e.g. via [`ConnectorContext`](crate::connector::ConnectorContext)),
    /// a `tracing` field, or a persisted error string never emits plaintext
    /// credentials. The `expires_at` timestamp and the *presence* (not value)
    /// of a refresh token are preserved as useful, non-secret context.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OAuth {
                access_token: _,
                refresh_token,
                expires_at,
                client_secret,
            } => f
                .debug_struct("SecretBundle::OAuth")
                .field("access_token", &"<redacted>")
                .field(
                    "refresh_token",
                    &refresh_token.as_ref().map(|_| "<redacted>"),
                )
                .field(
                    "client_secret",
                    &client_secret.as_ref().map(|_| "<redacted>"),
                )
                .field("expires_at", expires_at)
                .finish(),
            Self::ApiToken { token: _ } => f
                .debug_struct("SecretBundle::ApiToken")
                .field("token", &"<redacted>")
                .finish(),
            Self::AppPassword { password: _ } => f
                .debug_struct("SecretBundle::AppPassword")
                .field("password", &"<redacted>")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_bundle_debug_redacts_all_secret_values() {
        let bundles = [
            SecretBundle::OAuth {
                access_token: "super-secret-access".into(),
                refresh_token: Some("super-secret-refresh".into()),
                expires_at: None,
                client_secret: Some("super-secret-client".into()),
            },
            SecretBundle::OAuth {
                access_token: "a".into(),
                refresh_token: None,
                expires_at: None,
                client_secret: None,
            },
            SecretBundle::ApiToken {
                token: "super-secret-token".into(),
            },
            SecretBundle::AppPassword {
                password: "super-secret-password".into(),
            },
        ];
        for bundle in &bundles {
            let dbg = format!("{bundle:?}");
            assert!(
                !dbg.contains("super-secret"),
                "Debug leaked a secret value: {dbg}"
            );
            // The discriminant is preserved (useful, non-secret context).
            assert!(
                dbg.contains("SecretBundle::"),
                "missing discriminant: {dbg}"
            );
        }
        // The presence of a refresh token is shown without its value.
        let with_rt = format!(
            "{:?}",
            SecretBundle::OAuth {
                access_token: "a".into(),
                refresh_token: Some("rt".into()),
                expires_at: None,
                client_secret: None,
            }
        );
        assert!(
            with_rt.contains("<redacted>"),
            "must redact refresh_token: {with_rt}"
        );
        let without_rt = format!(
            "{:?}",
            SecretBundle::OAuth {
                access_token: "a".into(),
                refresh_token: None,
                expires_at: None,
                client_secret: Some("super-secret-client".into()),
            }
        );
        assert!(
            without_rt.contains("None"),
            "must show refresh_token absence: {without_rt}"
        );
    }
}
