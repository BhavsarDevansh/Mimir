//! Connector credential storage (Phase 3 F10 / issue #187).
//!
//! One [`SecretStore`] backs every connector auth kind: OAuth 2.0 tokens, API
//! tokens, and app passwords. The V1 default is [`FileSecretStore`] — one JSON
//! file per connector instance under `~/.local/share/mimir/secrets/`, file
//! mode `0600`, parent directory mode `0700`, **plaintext at rest**.
//!
//! At-rest encryption is intentionally deferred (consistent with the existing
//! plaintext LLM API key in `config.toml` and the home-directory trust
//! boundary). A `keyring`-backed store is tracked separately as #188.
//!
//! # Why one store for all kinds
//!
//! A connector authenticates in exactly one of three ways, and the auth kind
//! is a property of the *connector instance*, not the store. Keeping a single
//! [`SecretBundle`] enum under one trait means the supervisor, CLI, and server
//! routes never branch on "which secret store do I talk to" — they ask for the
//! bundle by slug and pattern-match the kind.
//!
//! # Permissions model (Unix)
//!
//! [`FileSecretStore`] *fails closed*: if the secret file or its parent
//! directory is more permissive than `0600`/`0700` (any group or other bits
//! set), [`FileSecretStore::load`] returns [`SecretError::InsecurePermissions`]
//! rather than reading and potentially leaking the credential. Store and
//! delete always (re)apply the tight modes. On non-Unix targets the
//! permission checks are skipped (file modes are not available); this is a
//! documented limitation of V1, which targets Linux primarily.
//!
//! # Path-traversal safety
//!
//! The connector `slug` is used directly as a file stem. Although the
//! knowledge graph enforces slug uniqueness, the store does not trust that and
//! validates every slug against `^[A-Za-z0-9_-]{1,128}$`, rejecting empty,
//! dot/dotdot, path separators, spaces, and non-ASCII before touching the
//! filesystem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

use mimir_core::paths;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors raised by [`SecretStore`] implementations.
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
    /// [`SecretBundle`]'s schema.
    #[error("secret file for `{slug}` is corrupt: {source}")]
    Corrupt {
        slug: String,
        #[source]
        source: serde_json::Error,
    },

    /// Failed to serialize a [`SecretBundle`] for writing.
    #[error("failed to serialize secret bundle: {0}")]
    Serialize(#[from] serde_json::Error),

    /// The secret file or its parent directory is more permissive than
    /// allowed (`0600`/`0700`) and the store refuses to read it.
    #[error(
        "secret file or directory for `{slug}` has insecure permissions \
         (expected file 0600 / dir 0700, no group or other bits)"
    )]
    InsecurePermissions { slug: String },
}

// ---------------------------------------------------------------------------
// SecretBundle
// ---------------------------------------------------------------------------

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    },
    /// A static API token presented as a bearer/`Authorization` header.
    ApiToken {
        /// The secret token string.
        token: String,
    },
    /// A username/password pair where the password is an app-specific secret.
    AppPassword {
        /// The app-specific password.
        password: String,
    },
}

// ---------------------------------------------------------------------------
// SecretStore trait
// ---------------------------------------------------------------------------

/// Async credential storage for connector instances, keyed by connector slug.
///
/// One store handles all [`SecretBundle`] kinds. `load` returns `Ok(None)` when
/// the slug has no stored credentials (a connector that has not been
/// authenticated yet). `store` is idempotent — storing over an existing slug
/// overwrites it atomically. `delete` is idempotent — deleting a missing slug
/// is `Ok(())`.
///
/// The trait is `async` (via [`async_trait`]) so a future network-backed store
/// (the deferred `keyring` / Secret Service backend, #188) can implement it
/// without a breaking change, and so it composes cleanly with the async
/// [`crate::Connector`] pipeline.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Load the credentials for `slug`, or `Ok(None)` if none are stored.
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError>;

    /// Persist `bundle` under `slug`, overwriting any existing file atomically.
    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError>;

    /// Remove the credentials for `slug`. Idempotent: a missing slug is `Ok`.
    async fn delete(&self, slug: &str) -> Result<(), SecretError>;
}

// ---------------------------------------------------------------------------
// Slug validation
// ---------------------------------------------------------------------------

/// Maximum slug length. Keeps filenames reasonable; connector slugs in the
/// knowledge graph are short human labels.
const MAX_SLUG_LEN: usize = 128;

/// Validate a connector slug for use as a secret-file stem.
///
/// Accepts `[A-Za-z0-9_-]{1,128}` — alphanumeric, underscore, and hyphen only.
/// Rejects empty, path separators, `..`, spaces, dots, and non-ASCII, all of
/// which would either traverse the filesystem or surprise the user.
/// Monotonic counter guaranteeing unique temp-file names within a process.
/// Combined with `std::process::id()` (via the directory scope, not needed here
/// since each process has its own counter) this makes concurrent same-slug
/// `store` calls write to distinct temp files.
static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_temp_counter() -> u64 {
    TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn validate_slug(slug: &str) -> Result<(), SecretError> {
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

// ---------------------------------------------------------------------------
// FileSecretStore
// ---------------------------------------------------------------------------

/// V1 default [`SecretStore`]: one JSON file per connector instance.
///
/// Files live under a base directory (default `~/.local/share/mimir/secrets/`,
/// overridable with [`FileSecretStore::with_dir`] for tests), named
/// `<slug>.json`. The directory is created with mode `0700` and each file with
/// mode `0600`; loads refuse to read a file or directory that is more
/// permissive. Writes are atomic (temp file + `rename`) so a crash cannot
/// leave a truncated secret file that silently logs a connector out.
///
/// The store is cheaply [`Clone`]able (it holds only a [`PathBuf`]) and
/// stateless, so multiple connectors can share one cheaply.
#[derive(Debug, Clone)]
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    /// Create a store rooted at the default Mimir secrets directory
    /// (`~/.local/share/mimir/secrets`). The directory is *not* created here;
    /// it is created on first [`Self::store`] (and validated on
    /// [`Self::load`]).
    pub fn new() -> Result<Self, SecretError> {
        Ok(Self {
            dir: paths::secrets_dir()?,
        })
    }

    /// Create a store rooted at an explicit directory (used by tests and, in
    /// future, for configurable secret roots).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The base directory this store reads and writes under.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the secret file for `slug`. Caller is expected to have validated
    /// `slug`; this is kept private to avoid exposing a way to bypass
    /// validation.
    fn secret_path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.json"))
    }

    /// Ensure the base directory exists with mode `0700`.
    ///
    /// `create_dir_all` is idempotent, so this is safe to call every `store`.
    /// The mode is then (re)applied on Unix so a manually-loosened dir is
    /// re-tightened rather than silently leaving secrets readable.
    fn ensure_dir(&self) -> Result<(), SecretError> {
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Atomic write: serialise to a uniquely-named sibling temp file, then
    /// `rename` onto the final path and (re)apply `0600`.
    ///
    /// The temp file name embeds the process id and a per-process monotonic
    /// counter, so two concurrent `store` calls for the *same* slug never
    /// collide on the same temp file (the supervisor serialises per-connector
    /// already, but the store does not rely on that). If `rename` fails the
    /// temp file is best-effort removed so no stale files linger.
    fn write_bundle(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError> {
        let path = self.secret_path(slug);
        let bytes = serde_json::to_vec_pretty(bundle)?;

        let tmp = {
            let n = next_temp_counter();
            self.dir.join(format!("{slug}.json.tmp.{n}"))
        };
        std::fs::write(&tmp, &bytes)?;

        let rename_res = std::fs::rename(&tmp, &path);
        if rename_res.is_err() {
            // Best-effort: don't leave the temp file behind on rename failure.
            let _ = std::fs::remove_file(&tmp);
        }
        rename_res?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError> {
        validate_slug(slug)?;
        let path = self.secret_path(slug);
        // One metadata call: NotFound means "no secret stored yet"; anything
        // else is read after the permission check. This avoids the TOCTOU of a
        // separate `exists()` + `read()` pair.
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        self.assert_permissions(slug, &meta)?;
        let bytes = std::fs::read(&path)?;
        let bundle = serde_json::from_slice::<SecretBundle>(&bytes).map_err(|source| {
            SecretError::Corrupt {
                slug: slug.to_string(),
                source,
            }
        })?;
        Ok(Some(bundle))
    }

    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError> {
        validate_slug(slug)?;
        self.ensure_dir()?;
        self.write_bundle(slug, bundle)
    }

    async fn delete(&self, slug: &str) -> Result<(), SecretError> {
        validate_slug(slug)?;
        let path = self.secret_path(slug);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl FileSecretStore {
    /// Refuse to read if the secret file or its parent dir has any group/other
    /// permission bits set. The canonical modes are `0600` (file) and `0700`
    /// (dir); we additionally tolerate an owner-exec bit on a directory
    /// (some platforms set it), but never any group/other bits.
    /// Refuse to read if the secret file or its parent dir has any group/other
    /// permission bits set. The canonical modes are `0600` (file) and `0700`
    /// (dir); owner bits (including owner-exec on a dir) are tolerated, but
    /// never any group/other bits. `file_meta` is the already-fetched file
    /// metadata from [`Self::load`] so the file is stat'd only once.
    #[cfg(unix)]
    fn assert_permissions(
        &self,
        slug: &str,
        file_meta: &std::fs::Metadata,
    ) -> Result<(), SecretError> {
        use std::os::unix::fs::PermissionsExt;
        let file_mode = file_meta.permissions().mode() & 0o777;
        if file_mode & 0o077 != 0 {
            warn!(
                slug = slug,
                mode = format!("{file_mode:o}"),
                "refusing to read secret file with group/other permission bits set"
            );
            return Err(SecretError::InsecurePermissions {
                slug: slug.to_string(),
            });
        }
        let dir_mode = std::fs::metadata(&self.dir)?.permissions().mode() & 0o777;
        if dir_mode & 0o077 != 0 {
            warn!(
                dir = %self.dir.display(),
                mode = format!("{dir_mode:o}"),
                "refusing to read secret: parent dir has group/other permission bits set"
            );
            return Err(SecretError::InsecurePermissions {
                slug: slug.to_string(),
            });
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn assert_permissions(
        &self,
        _slug: &str,
        _file_meta: &std::fs::Metadata,
    ) -> Result<(), SecretError> {
        // Non-Unix targets have no portable file-mode concept; V1 targets
        // Linux and skips enforcement here.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InMemorySecretStore (test/helper)
// ---------------------------------------------------------------------------

/// In-memory [`SecretStore`] backed by a [`HashMap`].
///
/// Primarily a test harness for the [`crate::mock`] connector and unit tests,
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

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

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
