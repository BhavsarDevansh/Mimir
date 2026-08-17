//! In-memory [`SecretStore`] for tests and ephemeral processes.
//!
//! Backed by a `HashMap` behind a `Mutex`; primarily a test harness for the
//! [`crate::mock`] connector, but usable by any caller that does not want
//! on-disk persistence.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::bundle::SecretBundle;
use super::error::SecretError;
use super::store::{SecretStore, validate_slug};

/// In-memory [`SecretStore`] backed by a [`HashMap`].
///
/// Primarily a test harness for the `crate::mock` connector and unit tests,
/// but exposed so any caller that does not want on-disk persistence (e.g. an
/// ephemeral daemon process) can use it directly. Thread-safe via a [`Mutex`]; wrap in `Arc` to share across tasks.
#[derive(Debug, Default)]
pub struct InMemorySecretStore {
    map: Mutex<HashMap<String, SecretBundle>>,
}

impl InMemorySecretStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SecretStore for InMemorySecretStore {
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError> {
        validate_slug(slug)?;
        Ok(self.map.lock().unwrap().get(slug).cloned())
    }

    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError> {
        validate_slug(slug)?;
        self.map
            .lock()
            .unwrap()
            .insert(slug.to_string(), bundle.clone());
        Ok(())
    }

    async fn delete(&self, slug: &str) -> Result<(), SecretError> {
        validate_slug(slug)?;
        self.map.lock().unwrap().remove(slug);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_store_debug_redacts_bundled_secrets() {
        let store = InMemorySecretStore::new();
        store
            .store(
                "cal-google",
                &SecretBundle::ApiToken {
                    token: "super-secret-token".into(),
                },
            )
            .await
            .unwrap();
        let dbg = format!("{store:?}");
        assert!(
            !dbg.contains("super-secret"),
            "store Debug leaked a secret: {dbg}"
        );
        assert!(
            dbg.contains("SecretBundle::ApiToken"),
            "must keep discriminant: {dbg}"
        );
    }
}
