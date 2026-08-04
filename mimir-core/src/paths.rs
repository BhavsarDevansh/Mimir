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

/// Returns the systemd user service directory (`~/.config/systemd/user` on Linux).
pub fn systemd_user_dir() -> Result<PathBuf, PathsError> {
    dirs::config_dir()
        .map(|p| p.join("systemd").join("user"))
        .ok_or(PathsError::MissingConfigDir)
}

/// Returns the path to `config.toml` inside the config directory.
pub fn config_path() -> Result<PathBuf, PathsError> {
    config_dir().map(|p| p.join("config.toml"))
}

/// Returns the path to the context database inside the data directory.
pub fn default_db_path() -> Result<PathBuf, PathsError> {
    data_dir().map(|p| p.join("context.db"))
}
/// Returns the path to the knowledge graph database inside the data directory.
pub fn knowledge_db_path() -> Result<PathBuf, PathsError> {
    data_dir().map(|p| p.join("knowledge.db"))
}

/// Returns the path to the shared job queue database inside the data directory.
pub fn jobs_db_path() -> Result<PathBuf, PathsError> {
    data_dir().map(|p| p.join("jobs.db"))
}

/// Resolve a database path from an optional config override, falling back to
/// a default resolver when unset. Used by `AppState` to honour the
/// `context.db_path` / `knowledge.db_path` / `scheduler.db_path` overrides
/// while preserving the error from the default resolver (issue #233).
pub fn resolve_db_path(
    override_path: Option<PathBuf>,
    default: impl FnOnce() -> Result<PathBuf, PathsError>,
) -> Result<PathBuf, PathsError> {
    match override_path {
        Some(p) => Ok(p),
        None => default(),
    }
}

/// Returns the path to the connector secrets directory inside the data
/// directory (`~/.local/share/mimir/secrets` on Linux).
///
/// One JSON file per connector instance lives here, keyed by the connector
/// slug (see [`secrets_file`]). The directory is created with mode `0700` by
/// `FileSecretStore`; this helper only *resolves* the path.
pub fn secrets_dir() -> Result<PathBuf, PathsError> {
    data_dir().map(|p| p.join("secrets"))
}

/// Returns the path to a single connector's secret file inside the secrets
/// directory, i.e. `<secrets_dir>/<slug>.json`.
///
/// `slug` is used verbatim as a file stem; callers (the secret store) are
/// responsible for validating it against path-traversal characters.
pub fn secrets_file(slug: &str) -> Result<PathBuf, PathsError> {
    secrets_dir().map(|p| p.join(format!("{slug}.json")))
}

/// Returns the path to the user skills directory inside the config directory.
pub fn skills_dir() -> Result<PathBuf, PathsError> {
    config_dir().map(|p| p.join("skills"))
}

/// Returns the path to the REPL history file inside the config directory.
pub fn history_path() -> Result<PathBuf, PathsError> {
    config_dir().map(|p| p.join("history.txt"))
}

/// Returns the path to the user personalities directory inside the config directory.
pub fn personalities_dir() -> Result<PathBuf, PathsError> {
    config_dir().map(|p| p.join("personalities"))
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

    #[test]
    fn test_systemd_user_dir_ends_with_systemd_user() {
        let dir = systemd_user_dir().unwrap();
        let s = dir.to_string_lossy();
        assert!(
            s.ends_with("systemd/user"),
            "expected systemd/user suffix, got: {}",
            s
        );
    }

    #[test]
    fn test_knowledge_db_path_is_data_dir_plus_db() {
        let path = knowledge_db_path().unwrap();
        assert!(path.ends_with("mimir/knowledge.db"));
    }

    #[test]
    fn test_jobs_db_path_is_data_dir_plus_db() {
        let path = jobs_db_path().unwrap();
        assert!(path.ends_with("mimir/jobs.db"));
    }

    #[test]
    fn test_skills_dir_is_config_dir_plus_skills() {
        let path = skills_dir().unwrap();
        assert!(path.ends_with("mimir/skills"));
    }

    #[test]
    fn test_history_path_is_config_dir_plus_history_txt() {
        let path = history_path().unwrap();
        assert!(path.ends_with("mimir/history.txt"));
    }

    #[test]
    fn test_personalities_dir_is_config_dir_plus_personalities() {
        let path = personalities_dir().unwrap();
        assert!(path.ends_with("mimir/personalities"));
    }

    #[test]
    fn test_secrets_dir_is_data_dir_plus_secrets() {
        let path = secrets_dir().unwrap();
        assert!(path.ends_with("mimir/secrets"));
    }

    #[test]
    fn test_secrets_file_is_secrets_dir_plus_slug_json() {
        let path = secrets_file("gmail-personal").unwrap();
        assert!(path.ends_with("mimir/secrets/gmail-personal.json"));
    }
}
