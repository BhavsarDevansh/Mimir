//! Transport selection for local CLI↔daemon communication (issue #25).

#[cfg(unix)]
use std::path::PathBuf;

/// How a CLI command reaches the daemon.
///
/// Local commands prefer the Unix domain socket (instant daemon detection via
/// a local connection attempt, filesystem-permission access control); the TCP
/// transport remains for explicit remote endpoints (`MIMIR_BASE_URL`) and
/// non-Unix platforms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonTransport {
    /// Unix domain socket at `<data_dir>/mimir.sock` or a configured path.
    #[cfg(unix)]
    Unix(PathBuf),
    /// HTTP base URL (e.g. `http://127.0.0.1:8080`).
    Tcp(String),
}

impl DaemonTransport {
    /// Resolve the transport for this CLI invocation.
    ///
    /// Precedence:
    /// 1. `MIMIR_BASE_URL` — an explicit remote/alternate daemon wins over the
    ///    local socket.
    /// 2. Unix socket: `MIMIR_SERVER_SOCKET_PATH` → `server.socket_path` in
    ///    the config file → the default (`<data_dir>/mimir.sock`).
    /// 3. TCP fallback to [`crate::constants::base_url`] (config `bind_addr`
    ///    or the compiled default).
    pub fn resolve() -> DaemonTransport {
        Self::resolve_from(
            std::env::var("MIMIR_BASE_URL").ok(),
            mimir_core::config::configured_socket_path().as_deref(),
            &crate::constants::base_url(),
        )
    }

    /// Pure resolution so the precedence is unit-testable without mutating
    /// the process environment (the `std::env::set_var` ban, Rust 2024).
    fn resolve_from(
        env_base_url: Option<String>,
        configured_socket_path: Option<&str>,
        tcp_fallback: &str,
    ) -> DaemonTransport {
        if let Some(url) = env_base_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return DaemonTransport::Tcp(url);
        }
        #[cfg(unix)]
        if let Some(path) = mimir_core::config::effective_socket_path(configured_socket_path) {
            return DaemonTransport::Unix(path);
        }
        #[cfg(not(unix))]
        let _ = configured_socket_path;
        DaemonTransport::Tcp(tcp_fallback.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_base_url_wins_over_local_socket() {
        let transport = DaemonTransport::resolve_from(
            Some("http://remote:9000".to_string()),
            Some("~/mimir.sock"),
            "http://127.0.0.1:8080",
        );
        assert_eq!(
            transport,
            DaemonTransport::Tcp("http://remote:9000".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_socket_wins_over_tcp_fallback() {
        let home = dirs::home_dir().expect("home dir must resolve in tests");
        let transport =
            DaemonTransport::resolve_from(None, Some("~/mimir.sock"), "http://127.0.0.1:8080");
        assert_eq!(transport, DaemonTransport::Unix(home.join("mimir.sock")));
    }

    #[cfg(unix)]
    #[test]
    fn unset_socket_defaults_to_unix_socket() {
        let default_socket = mimir_core::config::effective_socket_path(None).unwrap();
        let transport = DaemonTransport::resolve_from(None, None, "http://127.0.0.1:8080");
        assert_eq!(transport, DaemonTransport::Unix(default_socket.clone()));
        let blank = DaemonTransport::resolve_from(None, Some("   "), "http://127.0.0.1:8080");
        assert_eq!(blank, DaemonTransport::Unix(default_socket));
    }
}
