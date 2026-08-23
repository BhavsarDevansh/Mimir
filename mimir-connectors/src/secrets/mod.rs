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
//!
//! The module is split by concern:
//!
//! - `error` — [`SecretError`].
//! - `bundle` — [`SecretBundle`] and its redacted `Debug` impl.
//! - `store` — the [`SecretStore`] trait and shared slug validation.
//! - `file` — [`FileSecretStore`], the V1 on-disk store.
//! - `memory` — [`InMemorySecretStore`], the test/helper store.
//! - `keyring` — [`KeyringSecretStore`], the opt-in OS-keychain store (F11 /
//!   #188, feature `secrets-keyring`, off by default).

mod bundle;
mod error;
mod file;
mod memory;
mod store;

#[cfg(all(
    feature = "secrets-keyring",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod keyring;

#[cfg(any(feature = "calendar", feature = "gmail"))]
use crate::connector::ConnectorError;

pub use bundle::SecretBundle;
pub use error::SecretError;
pub use file::FileSecretStore;
#[cfg(all(
    feature = "secrets-keyring",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use keyring::KeyringSecretStore;
pub use memory::InMemorySecretStore;
pub use store::SecretStore;

/// Shared auth-kind discriminant contract (issue #341).
///
/// Both `CalendarAuthMethod` and `EmailAuthMethod` implement this trait so
/// the non-secret `kind` strings cannot drift between the two connectors: a
/// new auth variant is forced to map here, and `mismatch_error` callers pass
/// `auth.discriminant()`.
#[cfg(any(feature = "calendar", feature = "gmail"))]
pub(crate) trait AuthMethodDiscriminant {
    /// The non-secret discriminant name (the serde `kind` tag), for error
    /// messages that must not `Debug`-format the OAuth `client_secret`.
    fn discriminant(&self) -> &'static str;
}

/// Build the `Authentication` error raised when a connector's configured auth
/// method does not match the stored [`SecretBundle`] kind (issue #273).
///
/// Shared by the Calendar and Email `resolve_auth` matches so the message and
/// the `discriminant()` value stay in sync across both connectors. Only the
/// non-secret auth-kind discriminant is included — never a `Debug` of the
/// OAuth config (which could echo the client secret).
#[cfg(any(feature = "calendar", feature = "gmail"))]
pub(crate) fn mismatch_error(config_discriminant: &str) -> ConnectorError {
    ConnectorError::Authentication(format!(
        "auth method {} does not match stored secret kind",
        config_discriminant
    ))
}

#[cfg(all(test, any(feature = "calendar", feature = "gmail")))]
#[path = "mod_tests.rs"]
mod tests;
