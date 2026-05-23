//! Platform-specific path resolution for Mimir.
//!
//! All directory and file paths are resolved via the `dirs` crate, which
//! honours XDG base-directory conventions on Linux, and platform equivalents
//! on macOS and Windows.

use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur while resolving Mimir paths.
#[derive(Debug, Error)]
pub enum PathsError {
    /// The platform configuration directory could not be determined.
    #[error(
        "Could not determine config directory. \
        Ensure $HOME is set, or set $XDG_CONFIG_HOME to a valid path."
    )]
    MissingConfigDir,

    /// The platform data directory could not be determined.
    #[error(
        "Could not determine data directory. \
        Ensure $HOME is set, or set $XDG_DATA_HOME to a valid path."
    )]
    MissingDataDir,

    /// The platform cache directory could not be determined.
    #[error(
        "Could not determine cache directory. \
        Ensure $HOME is set, or set $XDG_CACHE_HOME to a valid path."
    )]
    MissingCacheDir,

    /// An I/O error occurred while creating directories.
    #[error("I/O error creating {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Returns the Mimir configuration directory (`~/.config/mimir` on Linux).
pub fn config_dir() -> Result<PathBuf, PathsError> {
    dirs::config_dir()
        .map(|p| p.join("mimir"))
        .ok_or(PathsError::MissingConfigDir)
}

/// Returns the Mimir data directory (`~/.local/share/mimir` on Linux).
pub fn data_dir() -> Result<PathBuf, PathsError> {
    dirs::data_dir()
        .map(|p| p.join("mimir"))
        .ok_or(PathsError::MissingDataDir)
}

/// Returns the Mimir cache directory (`~/.cache/mimir` on Linux).
pub fn cache_dir() -> Result<PathBuf, PathsError> {
    dirs::cache_dir()
        .map(|p| p.join("mimir"))
        .ok_or(PathsError::MissingCacheDir)
}

/// Returns the path to `config.toml` inside the config directory.
pub fn config_path() -> Result<PathBuf, PathsError> {
    config_dir().map(|p| p.join("config.toml"))
}

/// Returns the path to `memory.md` inside the config directory.
pub fn memory_path() -> Result<PathBuf, PathsError> {
    config_dir().map(|p| p.join("memory.md"))
}

/// Returns the path to the context database inside the data directory.
pub fn default_db_path() -> Result<PathBuf, PathsError> {
    data_dir().map(|p| p.join("context.db"))
}

/// Ensures a directory exists, creating it and all parents if needed.
///
/// Returns `Ok(())` if the directory already existed or was successfully created.
pub fn ensure_dir(path: &Path) -> Result<(), PathsError> {
    std::fs::create_dir_all(path).map_err(|source| PathsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_dir_ends_with_mimir() {
        let dir = config_dir().unwrap();
        assert!(dir.ends_with("mimir"));
    }

    #[test]
    fn test_data_dir_ends_with_mimir() {
        let dir = data_dir().unwrap();
        assert!(dir.ends_with("mimir"));
    }

    #[test]
    fn test_cache_dir_ends_with_mimir() {
        let dir = cache_dir().unwrap();
        assert!(dir.ends_with("mimir"));
    }

    #[test]
    fn test_config_path_is_config_dir_plus_toml() {
        let path = config_path().unwrap();
        assert!(path.ends_with("mimir/config.toml"));
    }

    #[test]
    fn test_memory_path_is_config_dir_plus_md() {
        let path = memory_path().unwrap();
        assert!(path.ends_with("mimir/memory.md"));
    }

    #[test]
    fn test_default_db_path_is_data_dir_plus_db() {
        let path = default_db_path().unwrap();
        assert!(path.ends_with("mimir/context.db"));
    }

    #[test]
    fn test_ensure_dir_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new_dir");
        assert!(!target.exists());
        ensure_dir(target.as_path()).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn test_ensure_dir_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new_dir");
        ensure_dir(target.as_path()).unwrap();
        ensure_dir(target.as_path()).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn test_ensure_dir_nested() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a").join("b").join("c");
        ensure_dir(target.as_path()).unwrap();
        assert!(target.is_dir());
    }
}
