use std::path::Path;

use serde::Deserialize;

use crate::paths;

// ---------------------------------------------------------------------------
// CLI base-URL resolution
// ---------------------------------------------------------------------------

/// Default base URL used by CLI clients when neither an environment override
/// nor a configured `server.bind_addr` is available.
pub const DEFAULT_CLI_BASE_URL: &str = "http://127.0.0.1:8080";

/// Build a daemon base URL for an HTTP client from a configured `bind_addr`.
///
/// Wildcard bind hosts (`0.0.0.0`, `[::]`) are normalised to their loopback
/// equivalents so the client connects locally rather than relying on the OS
/// wildcard-routing behaviour.
pub fn base_url_from_bind_addr(bind_addr: &str) -> String {
    let s = bind_addr.trim();
    let normalised = if let Some(rest) = s.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{rest}")
    } else if s.strip_prefix("[::]:").is_some() {
        s.replacen("[::]:", "[::1]:", 1)
    } else if s == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        s.to_string()
    };
    format!("http://{normalised}")
}

/// Resolve the daemon base URL for CLI clients.
///
/// Precedence (each tier falls through on a missing/blank value):
/// 1. Explicit environment override (`MIMIR_BASE_URL`).
/// 2. Configured `server.bind_addr` from the config file.
/// 3. Compiled default ([`DEFAULT_CLI_BASE_URL`]).
pub fn resolve_base_url(env_override: Option<&str>, configured_bind_addr: Option<&str>) -> String {
    if let Some(env) = env_override.map(str::trim).filter(|s| !s.is_empty()) {
        return env.to_string();
    }
    if let Some(bind) = configured_bind_addr
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return base_url_from_bind_addr(bind);
    }
    DEFAULT_CLI_BASE_URL.to_string()
}

/// Read `server.bind_addr` from the default config file.
///
/// Best-effort: returns `None` if the file is absent, unreadable,
/// unparseable, or omits the field. Never creates directories or writes
/// files, so it is safe to call from every CLI command (even before
/// `mimir init`).
pub fn configured_bind_addr() -> Option<String> {
    bind_addr_from_path(&paths::config_path().ok()?)
}

fn bind_addr_from_path(path: &Path) -> Option<String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct ServerOnly {
        bind_addr: Option<String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct ConfigOnly {
        server: ServerOnly,
    }
    let contents = std::fs::read_to_string(path).ok()?;
    let cfg: ConfigOnly = toml::from_str(&contents).ok()?;
    cfg.server.bind_addr.filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    mod base_url_tests {
        use super::*;

        #[test]
        fn test_base_url_from_bind_addr_passthrough() {
            assert_eq!(
                base_url_from_bind_addr("127.0.0.1:8080"),
                "http://127.0.0.1:8080"
            );
            assert_eq!(
                base_url_from_bind_addr("0.0.0.0:8008"),
                "http://127.0.0.1:8008"
            );
        }

        #[test]
        fn test_base_url_from_bind_addr_ipv6_wildcard() {
            assert_eq!(base_url_from_bind_addr("[::]:8008"), "http://[::1]:8008");
        }

        #[test]
        fn test_base_url_from_bind_addr_bare_wildcard() {
            assert_eq!(base_url_from_bind_addr("0.0.0.0"), "http://127.0.0.1");
        }

        #[test]
        fn test_base_url_from_bind_addr_trims_whitespace() {
            assert_eq!(
                base_url_from_bind_addr("  127.0.0.1:9999  "),
                "http://127.0.0.1:9999"
            );
        }

        #[test]
        fn test_resolve_base_url_env_wins() {
            assert_eq!(
                resolve_base_url(Some("http://example:1"), Some("127.0.0.1:8080")),
                "http://example:1"
            );
        }

        #[test]
        fn test_resolve_base_url_blank_env_falls_through() {
            assert_eq!(
                resolve_base_url(Some("   "), Some("0.0.0.0:8008")),
                "http://127.0.0.1:8008"
            );
        }

        #[test]
        fn test_resolve_base_url_config_used() {
            assert_eq!(
                resolve_base_url(None, Some("127.0.0.1:8008")),
                "http://127.0.0.1:8008"
            );
        }

        #[test]
        fn test_resolve_base_url_default() {
            assert_eq!(resolve_base_url(None, None), DEFAULT_CLI_BASE_URL);
            assert_eq!(resolve_base_url(None, Some("")), DEFAULT_CLI_BASE_URL);
        }

        #[test]
        fn test_bind_addr_from_path_reads_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(
                &path,
                "[server]\nbind_addr = \"0.0.0.0:8008\"\n[llm]\nmodel = \"gpt-4o\"\n",
            )
            .unwrap();
            assert_eq!(bind_addr_from_path(&path), Some("0.0.0.0:8008".to_string()));
        }

        #[test]
        fn test_bind_addr_from_path_missing_field() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "[llm]\nmodel = \"gpt-4o\"\n").unwrap();
            assert_eq!(bind_addr_from_path(&path), None);
        }

        #[test]
        fn test_bind_addr_from_path_missing_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("absent.toml");
            assert_eq!(bind_addr_from_path(&path), None);
        }

        #[test]
        fn test_bind_addr_from_path_unparseable() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "not valid toml [[").unwrap();
            assert_eq!(bind_addr_from_path(&path), None);
        }
    }
}
