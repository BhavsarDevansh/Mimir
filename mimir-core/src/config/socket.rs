//! Unix domain socket path resolution for the daemon and the CLI.

use std::path::PathBuf;

use crate::config::base_url::server_settings_from_file;
use crate::paths;

/// Resolve the effective Unix socket path from an optional configured value.
///
/// On Unix the configured value (tilde-expanded) wins; a blank or missing
/// value falls back to the platform default (`<data_dir>/mimir.sock`), so the
/// daemon and the CLI agree on the socket location without configuration.
/// On non-Unix platforms no Unix socket is available and `None` is returned.
#[cfg(unix)]
pub fn effective_socket_path(configured: Option<&str>) -> Option<PathBuf> {
    let path = configured
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(paths::expand_home)
        .unwrap_or(paths::socket_path().ok()?);
    Some(path)
}

/// No Unix socket on non-Unix platforms (Windows falls back to TCP).
#[cfg(not(unix))]
pub fn effective_socket_path(_configured: Option<&str>) -> Option<PathBuf> {
    None
}

/// Resolve the socket path the local CLI should connect to.
///
/// Precedence (each tier falls through on a missing/blank value):
/// 1. `MIMIR_SERVER_SOCKET_PATH` environment variable.
/// 2. `server.socket_path` from the default config file.
///
/// Best-effort: returns `None` when neither source is set. Never creates
/// directories or writes files, so it is safe to call from every CLI command
/// (even before `mimir init`).
pub fn configured_socket_path() -> Option<String> {
    let env_value = std::env::var("MIMIR_SERVER_SOCKET_PATH").ok();
    let file_value = socket_path_from_file();
    socket_path_from_sources(env_value, file_value.as_deref())
}

/// Pure env-over-file resolution so the precedence is unit-testable without
/// mutating process environment (the `std::env::set_var` ban, Rust 2024).
fn socket_path_from_sources(env_value: Option<String>, file_value: Option<&str>) -> Option<String> {
    env_value
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            file_value
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

/// Read `server.socket_path` from the default config file, sharing the
/// minimal `[server]`-section parser with the bind-address resolver.
fn socket_path_from_file() -> Option<String> {
    server_settings_from_file(&paths::config_path().ok()?)?.socket_path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn effective_path_expands_tilde_in_configured_value() {
        let home = dirs::home_dir().expect("home dir must resolve in tests");
        let path = effective_socket_path(Some("~/custom/mimir.sock")).unwrap();
        assert_eq!(path, home.join("custom").join("mimir.sock"));
    }

    #[cfg(unix)]
    #[test]
    fn effective_path_uses_absolute_configured_value_verbatim() {
        let path = effective_socket_path(Some("/tmp/mimir.sock")).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/mimir.sock"));
    }

    #[cfg(unix)]
    #[test]
    fn effective_path_defaults_to_data_dir_socket() {
        assert_eq!(effective_socket_path(None), paths::socket_path().ok());
        assert_eq!(
            effective_socket_path(Some("   ")),
            paths::socket_path().ok(),
            "blank configured values fall through to the default"
        );
    }

    #[test]
    fn env_value_wins_over_config_file_value() {
        let resolved =
            socket_path_from_sources(Some("~/env.sock".to_string()), Some("~/file.sock"));
        assert_eq!(resolved.as_deref(), Some("~/env.sock"));
    }

    #[test]
    fn blank_env_value_falls_through_to_config_file_value() {
        let resolved = socket_path_from_sources(Some("   ".to_string()), Some("~/file.sock"));
        assert_eq!(resolved.as_deref(), Some("~/file.sock"));
    }

    #[test]
    fn missing_sources_resolve_to_none() {
        assert_eq!(socket_path_from_sources(None, None), None);
        assert_eq!(socket_path_from_sources(None, Some("")), None);
        assert_eq!(
            socket_path_from_sources(Some(String::new()), Some("")),
            None
        );
    }
}
