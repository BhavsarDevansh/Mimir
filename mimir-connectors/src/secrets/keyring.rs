//! OS-keychain [`SecretStore`] backend (Phase 3 F11 / issue #188).
//!
//! [`KeyringSecretStore`] implements [`SecretStore`] over the `keyring` crate:
//! macOS Keychain, Linux/FreeBSD/OpenBSD Secret Service (gnome-keyring / KWallet over
//! D-Bus), and Windows Credential Manager. It is feature-gated behind
//! `secrets-keyring` (off by default — headless systemd boxes often lack a
//! Secret Service daemon, so the [`FileSecretStore`](super::file::FileSecretStore)
//! default remains the safe choice for those hosts).
//!
//! Every secret is stored under the keyring service `mimir` with the
//! connector slug as the account name, so each connector instance has exactly
//! one OS entry and the per-slug [`SecretStore`] contract maps 1:1 onto the
//! keyring entry model. The payload is the serialized [`SecretBundle`].

use async_trait::async_trait;
use std::sync::Arc;

use super::bundle::SecretBundle;
use super::error::SecretError;
use super::store::{SecretStore, validate_slug};

/// Keyring service name under which every connector secret is stored.
///
/// Keychain / Secret Service UIs group entries by service; `mimir` keeps all
/// connector credentials under one recognizable service with the connector
/// slug as the account (e.g. `gmail-personal`).
pub(crate) const KEYRING_SERVICE: &str = "mimir";

/// Platform credential-store operations behind [`KeyringSecretStore`].
///
/// The production implementation wraps the `keyring` crate's [`keyring::Entry`]
/// API; tests inject an in-memory backend so the store's behaviour (error
/// mapping, payload format, slug validation) is exercised headless. The
/// contract mirrors the OS stores exactly: a missing entry is
/// [`keyring::Error::NoEntry`] on both `get` and `delete`.
trait KeyringBackend: std::fmt::Debug + Send + Sync {
    /// Read the raw secret bytes for `(service, account)`, or
    /// [`keyring::Error::NoEntry`] when nothing is stored.
    fn get_secret(&self, service: &str, account: &str) -> Result<Vec<u8>, keyring::Error>;

    /// Store `secret` under `(service, account)`, replacing any existing
    /// entry.
    fn set_secret(&self, service: &str, account: &str, secret: &[u8])
    -> Result<(), keyring::Error>;

    /// Remove the entry for `(service, account)`, or
    /// [`keyring::Error::NoEntry`] when nothing is stored.
    fn delete_credential(&self, service: &str, account: &str) -> Result<(), keyring::Error>;
}

/// Production [`KeyringBackend`]: the `keyring` crate's cross-platform
/// `Entry` API.
#[derive(Debug)]
struct OsKeyringBackend;

impl KeyringBackend for OsKeyringBackend {
    fn get_secret(&self, service: &str, account: &str) -> Result<Vec<u8>, keyring::Error> {
        keyring::Entry::new(service, account)?.get_secret()
    }

    fn set_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), keyring::Error> {
        keyring::Entry::new(service, account)?.set_secret(secret)
    }

    fn delete_credential(&self, service: &str, account: &str) -> Result<(), keyring::Error> {
        keyring::Entry::new(service, account)?.delete_credential()
    }
}

/// [`SecretStore`] backed by the OS credential store via the `keyring` crate.
///
/// One OS entry per connector slug under the keyring service `KEYRING_SERVICE`,
/// holding the serialized [`SecretBundle`]. Construction is side-effect free;
/// the first operation connects to the platform store (Keychain / Secret
/// Service / Credential Manager) and surfaces availability problems as
/// [`SecretError::Keyring`] — there is no silent fallback to the file store,
/// because the user explicitly chose the keychain backend.
///
/// The `keyring` crate's `Entry` API is blocking (the Linux Secret Service
/// stack drives its D-Bus calls through async-io's `block_on`), so every
/// operation is dispatched through [`tokio::task::spawn_blocking`] onto a
/// dedicated blocking worker: the async `SecretStore` methods never occupy a
/// Tokio worker with blocking Keychain / D-Bus I/O, and never risk the
/// deadlock keyring warns about when its calls run on a tokio runtime thread.
#[derive(Debug)]
pub struct KeyringSecretStore {
    backend: Arc<dyn KeyringBackend>,
}

impl KeyringSecretStore {
    /// Create a store over the platform credential store.
    pub fn new() -> Self {
        Self {
            backend: Arc::new(OsKeyringBackend),
        }
    }

    /// Create a store over an explicit backend (headless unit tests).
    #[cfg(test)]
    fn with_backend(backend: Box<dyn KeyringBackend>) -> Self {
        Self {
            backend: Arc::from(backend),
        }
    }

    /// Run a blocking keyring operation on a dedicated tokio blocking worker.
    ///
    /// `keyring`'s `Entry` API blocks the calling thread even with
    /// `async-secret-service`, and keyring's own docs warn against calling it
    /// on a tokio runtime thread (deadlock risk), so every operation is
    /// dispatched via [`tokio::task::spawn_blocking`]. The only failure of the
    /// worker itself (panic or runtime shutdown) is surfaced as
    /// [`SecretError::KeyringTask`]; the backend result passes through
    /// unchanged.
    async fn run_blocking<T, F>(&self, op: F) -> Result<T, SecretError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn KeyringBackend) -> T + Send + 'static,
    {
        let backend = Arc::clone(&self.backend);
        tokio::task::spawn_blocking(move || op(backend.as_ref()))
            .await
            .map_err(SecretError::from)
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError> {
        validate_slug(slug)?;
        let account = slug.to_string();
        let result = self
            .run_blocking(move |backend| backend.get_secret(KEYRING_SERVICE, &account))
            .await?;
        match result {
            Ok(bytes) => {
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|source| SecretError::Corrupt {
                        slug: slug.to_string(),
                        source,
                    })
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(source) => Err(SecretError::Keyring(source)),
        }
    }

    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError> {
        validate_slug(slug)?;
        let bytes = serde_json::to_vec(bundle)?;
        let account = slug.to_string();
        let result = self
            .run_blocking(move |backend| backend.set_secret(KEYRING_SERVICE, &account, &bytes))
            .await?;
        result.map_err(SecretError::Keyring)
    }

    async fn delete(&self, slug: &str) -> Result<(), SecretError> {
        validate_slug(slug)?;
        let account = slug.to_string();
        let result = self
            .run_blocking(move |backend| backend.delete_credential(KEYRING_SERVICE, &account))
            .await?;
        match result {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(source) => Err(SecretError::Keyring(source)),
        }
    }
}

#[cfg(test)]
mod tests;
