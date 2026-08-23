//! Errors raised by secret store implementations.

use thiserror::Error;

use mimir_core::paths;

/// Errors raised by [`SecretStore`](super::store::SecretStore) implementations.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The connector slug is not a safe filename stem.
    #[error("invalid connector slug `{slug}`: {reason}")]
    InvalidSlug { slug: String, reason: String },

    /// The platform cannot resolve the Mimir data directory.
    #[error(transparent)]
    Paths(#[from] paths::PathsError),

    /// Local filesystem I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A secret file exists but is not valid JSON or does not match
    /// [`SecretBundle`](super::bundle::SecretBundle)'s schema.
    #[error("secret file for `{slug}` is corrupt: {source}")]
    Corrupt {
        slug: String,
        #[source]
        source: serde_json::Error,
    },

    /// Failed to serialize a [`SecretBundle`](super::bundle::SecretBundle) for writing.
    #[error("failed to serialize secret bundle: {0}")]
    Serialize(#[from] serde_json::Error),

    /// The secret file or its parent directory is more permissive than
    /// allowed (`0600`/`0700`) and the store refuses to read it.
    #[error(
        "secret file or directory for `{slug}` has insecure permissions \
         (expected file 0600 / dir 0700, no group or other bits)"
    )]
    InsecurePermissions { slug: String },

    /// The OS credential store (keyring) failed the operation.
    #[cfg(feature = "secrets-keyring")]
    #[error("OS keychain operation failed: {0}")]
    Keyring(#[from] keyring::Error),
}
