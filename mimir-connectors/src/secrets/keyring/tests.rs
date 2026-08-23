//! Headless unit tests for [`KeyringSecretStore`].
//!
//! The production backend talks to the OS credential store, which is not
//! available in CI or sandboxes (and must never be touched by tests), so
//! these tests inject an in-memory [`MemoryBackend`] that mirrors the OS
//! error contract: missing entries surface as `keyring::Error::NoEntry` and
//! `delete` of a missing entry fails with `NoEntry`, exactly like the Secret
//! Service / Keychain / Credential Manager stores.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use keyring::Error as KeyringError;

use super::{KEYRING_SERVICE, KeyringBackend, KeyringSecretStore};
use crate::secrets::bundle::SecretBundle;
use crate::secrets::error::SecretError;
use crate::secrets::store::SecretStore;

/// Stored `(service, account) → bytes` entries for [`MemoryBackend`].
type MemoryEntries = Arc<Mutex<HashMap<(String, String), Vec<u8>>>>;

/// In-memory `(service, account) → bytes` backend for headless tests.
///
/// `Arc`-backed so a test can keep a handle and inspect or pre-seed the
/// entries after handing the backend to the store.
#[derive(Debug, Clone, Default)]
struct MemoryBackend {
    entries: MemoryEntries,
}

impl KeyringBackend for MemoryBackend {
    fn get_secret(&self, service: &str, account: &str) -> Result<Vec<u8>, KeyringError> {
        self.entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), account.to_string()))
            .cloned()
            .ok_or(KeyringError::NoEntry)
    }

    fn set_secret(&self, service: &str, account: &str, secret: &[u8]) -> Result<(), KeyringError> {
        self.entries
            .lock()
            .unwrap()
            .insert((service.to_string(), account.to_string()), secret.to_vec());
        Ok(())
    }

    fn delete_credential(&self, service: &str, account: &str) -> Result<(), KeyringError> {
        let mut entries = self.entries.lock().unwrap();
        if entries
            .remove(&(service.to_string(), account.to_string()))
            .is_none()
        {
            return Err(KeyringError::NoEntry);
        }
        Ok(())
    }
}

/// Backend that always fails with a platform error, to exercise the
/// `SecretError::Keyring` mapping.
#[derive(Debug)]
struct FailingBackend;

impl KeyringBackend for FailingBackend {
    fn get_secret(&self, _service: &str, _account: &str) -> Result<Vec<u8>, KeyringError> {
        Err(KeyringError::PlatformFailure(Box::new(
            std::io::Error::other("keyring unavailable"),
        )))
    }

    fn set_secret(
        &self,
        _service: &str,
        _account: &str,
        _secret: &[u8],
    ) -> Result<(), KeyringError> {
        Err(KeyringError::PlatformFailure(Box::new(
            std::io::Error::other("keyring unavailable"),
        )))
    }

    fn delete_credential(&self, _service: &str, _account: &str) -> Result<(), KeyringError> {
        Err(KeyringError::PlatformFailure(Box::new(
            std::io::Error::other("keyring unavailable"),
        )))
    }
}

fn api_token_bundle() -> SecretBundle {
    SecretBundle::ApiToken {
        token: "tok-123".to_string(),
    }
}

fn app_password_bundle() -> SecretBundle {
    SecretBundle::AppPassword {
        password: "pw-456".to_string(),
    }
}

fn oauth_bundle() -> SecretBundle {
    SecretBundle::OAuth {
        access_token: "at-1".to_string(),
        refresh_token: Some("rt-1".to_string()),
        expires_at: Some(Utc::now()),
        client_secret: Some("cs-1".to_string()),
    }
}

fn test_store(backend: MemoryBackend) -> KeyringSecretStore {
    KeyringSecretStore::with_backend(Box::new(backend))
}

#[tokio::test]
async fn round_trips_all_bundle_kinds() {
    let store = test_store(MemoryBackend::default());
    let bundles = [
        ("gmail-oauth", oauth_bundle()),
        (
            "gmail-minimal",
            SecretBundle::OAuth {
                access_token: "at-2".to_string(),
                refresh_token: None,
                expires_at: None,
                client_secret: None,
            },
        ),
        ("home-assistant-api", api_token_bundle()),
        ("fastmail-app", app_password_bundle()),
    ];
    for (slug, bundle) in &bundles {
        store.store(slug, bundle).await.unwrap();
        assert_eq!(
            store.load(slug).await.unwrap().as_ref(),
            Some(bundle),
            "round trip failed for {slug}"
        );
    }
}

#[tokio::test]
async fn load_missing_slug_returns_none() {
    let store = test_store(MemoryBackend::default());
    assert_eq!(store.load("gmail-personal").await.unwrap(), None);
}

#[tokio::test]
async fn store_overwrites_existing_slug() {
    let store = test_store(MemoryBackend::default());
    store.store("gmail", &api_token_bundle()).await.unwrap();
    store.store("gmail", &app_password_bundle()).await.unwrap();
    assert_eq!(
        store.load("gmail").await.unwrap().as_ref(),
        Some(&app_password_bundle())
    );
}

#[tokio::test]
async fn delete_removes_bundle_and_is_idempotent() {
    let store = test_store(MemoryBackend::default());
    store.store("gmail", &api_token_bundle()).await.unwrap();
    store.delete("gmail").await.unwrap();
    assert_eq!(store.load("gmail").await.unwrap(), None);
    store.delete("gmail").await.unwrap();
}

#[tokio::test]
async fn stored_payload_is_the_serialized_bundle_json() {
    let backend = MemoryBackend::default();
    let store = test_store(backend.clone());
    let bundle = api_token_bundle();
    store.store("gmail", &bundle).await.unwrap();
    let entries = backend.entries.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries.get(&(KEYRING_SERVICE.to_string(), "gmail".to_string())),
        Some(&serde_json::to_vec(&bundle).unwrap())
    );
}

#[tokio::test]
async fn corrupt_payload_maps_to_corrupt_error_with_slug() {
    let backend = MemoryBackend::default();
    backend.entries.lock().unwrap().insert(
        (KEYRING_SERVICE.to_string(), "gmail".to_string()),
        b"not json".to_vec(),
    );
    let store = test_store(backend);
    match store.load("gmail").await {
        Err(SecretError::Corrupt { slug, .. }) => assert_eq!(slug, "gmail"),
        other => panic!("expected Corrupt error, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_slugs_are_rejected_before_touching_the_keyring() {
    let store = test_store(MemoryBackend::default());
    for slug in ["", "a/b", "..", "a b", "a.b", "ümlaut"] {
        assert!(
            matches!(
                store.store(slug, &api_token_bundle()).await,
                Err(SecretError::InvalidSlug { .. })
            ),
            "store must reject {slug:?}"
        );
        assert!(
            matches!(store.load(slug).await, Err(SecretError::InvalidSlug { .. })),
            "load must reject {slug:?}"
        );
        assert!(
            matches!(
                store.delete(slug).await,
                Err(SecretError::InvalidSlug { .. })
            ),
            "delete must reject {slug:?}"
        );
    }
}

#[tokio::test]
async fn platform_failure_maps_to_keyring_error() {
    let store = KeyringSecretStore::with_backend(Box::new(FailingBackend));
    assert!(matches!(
        store.load("gmail").await,
        Err(SecretError::Keyring(_))
    ));
    assert!(matches!(
        store.store("gmail", &api_token_bundle()).await,
        Err(SecretError::Keyring(_))
    ));
    assert!(matches!(
        store.delete("gmail").await,
        Err(SecretError::Keyring(_))
    ));
}
