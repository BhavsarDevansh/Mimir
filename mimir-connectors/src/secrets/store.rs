//! [`SecretStore`] trait and connector-slug validation shared by every store.
//!
//! The trait is deliberately small (load/store/delete, keyed by connector
//! slug) so a future network-backed store can implement it without a breaking
//! change. Slug validation lives here because both [`crate::secrets::file`]
//! and [`crate::secrets::memory`] must apply identical path-safety rules.

use async_trait::async_trait;

use super::bundle::SecretBundle;
use super::error::SecretError;

/// One store handles all [`SecretBundle`] kinds. `load` returns `Ok(None)` when
/// the slug has no stored credentials (a connector that has not been
/// authenticated yet). `store` is idempotent — storing over an existing slug
/// overwrites it atomically. `delete` is idempotent — deleting a missing slug
/// is `Ok(())`.
///
/// The trait is `async` (via [`async_trait::async_trait`]) so a future network-backed store
/// (the deferred `keyring` / Secret Service backend, #188) can implement it
/// without a breaking change, and so it composes cleanly with the async
/// [`crate::Connector`] pipeline.
#[async_trait]
pub trait SecretStore: Send + Sync + std::fmt::Debug {
    /// Load the credentials for `slug`, or `Ok(None)` if none are stored.
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError>;

    /// Persist `bundle` under `slug`, overwriting any existing file atomically.
    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError>;

    /// Remove the credentials for `slug`. Idempotent: a missing slug is `Ok`.
    async fn delete(&self, slug: &str) -> Result<(), SecretError>;
}

/// Maximum slug length. Keeps filenames reasonable; connector slugs in the
/// knowledge graph are short human labels.
const MAX_SLUG_LEN: usize = 128;

/// Validate a connector slug for use as a secret-file stem.
///
/// Accepts `[A-Za-z0-9_-]{1,128}` — alphanumeric, underscore, and hyphen only.
/// Rejects empty, path separators, `..`, spaces, dots, and non-ASCII, all of
/// which would either traverse the filesystem or surprise the user.
pub(super) fn validate_slug(slug: &str) -> Result<(), SecretError> {
    if slug.is_empty() {
        return Err(SecretError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not be empty".to_string(),
        });
    }
    if slug.len() > MAX_SLUG_LEN {
        return Err(SecretError::InvalidSlug {
            slug: slug.to_string(),
            reason: format!("slug longer than {MAX_SLUG_LEN} characters"),
        });
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(SecretError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug may contain only ASCII letters, digits, '_' and '-'".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_slug_accepts_valid() {
        for s in [
            "a",
            "gmail-personal",
            "Gmail_Personal",
            "cal1",
            "photos-2026",
        ] {
            assert!(validate_slug(s).is_ok(), "should accept `{s}`");
        }
    }

    #[test]
    fn validate_slug_rejects_invalid() {
        for s in ["", "..", "../etc", "a/b", "a b", "a.b", "café", "a:b"] {
            assert!(validate_slug(s).is_err(), "should reject `{s}`");
        }
    }

    #[test]
    fn validate_slug_rejects_overlong() {
        let s = "a".repeat(MAX_SLUG_LEN + 1);
        assert!(validate_slug(&s).is_err());
        let ok = "a".repeat(MAX_SLUG_LEN);
        assert!(validate_slug(&ok).is_ok());
    }
}
