//! Local API token management for the daemon HTTP API (issue #281).
//!
//! The daemon and the CLI authenticate each other with a shared bearer token
//! stored at `~/.local/share/mimir/api_token` (mode `0600`). The token is
//! generated on first use — at `mimir init`, at daemon startup, or by the
//! first CLI command — so existing installs are upgraded transparently.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::paths;

/// Errors that can occur while loading or creating the API token.
#[derive(Debug, Error)]
pub enum ApiTokenError {
    /// The platform data directory could not be resolved.
    #[error("could not resolve API token path: {0}")]
    Paths(#[from] paths::PathsError),

    /// An I/O error occurred while reading or writing the token file.
    #[error("I/O error accessing API token file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The token file exists but contains no usable token.
    #[error("API token file {path} is empty or contains only whitespace")]
    Empty { path: PathBuf },

    /// The operating system's random source failed.
    #[error("failed to generate a random API token: {0}")]
    Random(#[from] getrandom::Error),
}

/// Number of random bytes in a generated API token (256 bits of entropy).
const TOKEN_BYTES: usize = 32;

/// Returns the path to the daemon API token file
/// (`~/.local/share/mimir/api_token` on Linux).
pub fn api_token_path() -> Result<PathBuf, paths::PathsError> {
    paths::data_dir().map(|p| p.join("api_token"))
}

/// Load the API token from its default location, creating it if missing.
pub fn load_or_create_api_token() -> Result<String, ApiTokenError> {
    let path = api_token_path()?;
    load_or_create_api_token_at(&path)
}

/// Load the API token from `path`, creating it if missing.
///
/// The file is created with mode `0600` (Unix) and published atomically (the
/// token is written to a temporary file and then hard-linked into place), so
/// a concurrent creator race resolves to whichever process published first
/// and the loser re-reads the winner's complete file. The returned token is
/// always the canonical one on disk, so every caller ends up with the same
/// token. An existing file is never overwritten, so a user-supplied token is
/// preserved.
pub fn load_or_create_api_token_at(path: &Path) -> Result<String, ApiTokenError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let token = contents.trim();
            if token.is_empty() {
                return Err(ApiTokenError::Empty {
                    path: path.to_path_buf(),
                });
            }
            warn_on_loose_permissions(path);
            Ok(token.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let token = generate_token()?;
            write_token_file(path, &token)
        }
        Err(e) => Err(ApiTokenError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Compare a presented token against the expected token in constant time.
///
/// Used by the server's auth middleware so comparison time does not depend
/// on the matching prefix.
pub fn verify_api_token(presented: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// Generate a fresh 256-bit token, hex-encoded.
fn generate_token() -> Result<String, ApiTokenError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)?;
    Ok(hex_encode(&bytes))
}

/// Write `token` to `path` with `0600` permissions, creating parent
/// directories as needed.
///
/// The token is first written to a unique temporary file in the same
/// directory and then published with an atomic `hard_link`, so a concurrent
/// creator race resolves to whichever process published first and the loser
/// re-reads the winner's complete file (a direct `create_new` write would
/// let the loser observe a partially written token). If the file already
/// exists (a creation race or a user-supplied token), it is re-read so both
/// processes agree on the same token. Filesystems without hard-link support
/// fall back to a direct `create_new` write.
///
/// Returns the canonical token: the one published by this call, or the
/// winner's token re-read from the file when a concurrent creator won the
/// race.
fn write_token_file(path: &Path, token: &str) -> Result<String, ApiTokenError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ApiTokenError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let tmp_name = format!(
        "api_token.tmp.{}.{}",
        std::process::id(),
        &token[..token.len().min(8)]
    );
    let tmp_path = path
        .parent()
        .map_or_else(|| PathBuf::from(&tmp_name), |parent| parent.join(&tmp_name));
    if let Err(source) = write_token_to(&tmp_path, token) {
        if source.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(&tmp_path);
        }
        return Err(ApiTokenError::Io {
            path: tmp_path,
            source,
        });
    }

    match std::fs::hard_link(&tmp_path, path) {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp_path);
            Ok(token.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp_path);
            load_or_create_api_token_at(path)
        }
        Err(_) => {
            let _ = std::fs::remove_file(&tmp_path);
            write_token_direct(path, token)
        }
    }
}

/// Write `token` to `path` with `0600` permissions using `create_new`
/// semantics, so an existing file is never overwritten.
fn write_token_to(path: &Path, token: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(token.as_bytes())
}

/// Fallback for filesystems without hard-link support: create the target
/// directly with `create_new` semantics. The atomic-publish guarantee is
/// lost on such filesystems, but they are rare for a user data directory.
fn write_token_direct(path: &Path, token: &str) -> Result<String, ApiTokenError> {
    match write_token_to(path, token) {
        Ok(()) => Ok(token.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            load_or_create_api_token_at(path)
        }
        Err(e) => Err(ApiTokenError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Log a warning when an existing token file has group/other permissions,
/// since the token is only as secret as the file that stores it.
#[cfg(unix)]
fn warn_on_loose_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            tracing::warn!(
                "API token file {} has permissions {mode:o}; expected 600",
                path.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_on_loose_permissions(_path: &Path) {}

/// Lowercase hex encoding for the generated token bytes.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_token_file_with_0600_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");

        let token = load_or_create_api_token_at(&path).unwrap();

        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "token file must be owner-only");
        }
    }

    #[test]
    fn second_load_returns_same_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");

        let first = load_or_create_api_token_at(&path).unwrap();
        let second = load_or_create_api_token_at(&path).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn existing_file_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");
        std::fs::write(&path, "user-supplied-token\n").unwrap();

        let token = load_or_create_api_token_at(&path).unwrap();

        assert_eq!(token, "user-supplied-token");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "user-supplied-token\n"
        );
    }

    #[test]
    fn empty_file_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");
        std::fs::write(&path, "   \n").unwrap();

        let result = load_or_create_api_token_at(&path);

        assert!(matches!(result, Err(ApiTokenError::Empty { .. })));
    }

    #[test]
    fn creates_missing_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("dir").join("api_token");

        let token = load_or_create_api_token_at(&path).unwrap();

        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(path.is_file());
    }

    #[test]
    fn creation_leaves_no_temporary_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");

        load_or_create_api_token_at(&path).unwrap();

        let entries: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("api_token")]);
    }

    #[test]
    fn creation_race_returns_winner_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");
        std::fs::write(&path, "winner-token\n").unwrap();

        let token = write_token_file(&path, "loser-token").unwrap();

        assert_eq!(token, "winner-token");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "winner-token\n");
    }

    #[test]
    fn concurrent_creators_agree_on_canonical_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || load_or_create_api_token_at(&path).unwrap())
            })
            .collect();
        let tokens: Vec<String> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(tokens.iter().all(|token| *token == on_disk));
    }

    #[test]
    fn concurrent_readers_never_observe_partial_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("api_token");

        let creator = {
            let path = path.clone();
            std::thread::spawn(move || load_or_create_api_token_at(&path).unwrap())
        };
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        if let Ok(contents) = std::fs::read_to_string(&path) {
                            let token = contents.trim();
                            assert!(
                                token.len() == TOKEN_BYTES * 2
                                    && token.chars().all(|c| c.is_ascii_hexdigit()),
                                "reader observed a partial token: {token:?}"
                            );
                        }
                    }
                })
            })
            .collect();

        creator.join().unwrap();
        for reader in readers {
            reader.join().unwrap();
        }
    }

    #[test]
    fn verify_api_token_accepts_match_and_rejects_mismatch() {
        let expected = "0123456789abcdef";
        assert!(verify_api_token(expected, expected));
        assert!(!verify_api_token("0123456789abcdee", expected));
        assert!(!verify_api_token("", expected));
        assert!(!verify_api_token("0123456789abcdef00", expected));
    }
}
