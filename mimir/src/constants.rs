//! Shared constants for the mimir CLI binary.

use std::borrow::Cow;
use std::sync::LazyLock;

use mimir_core::config;

/// Default URL for the local Mimir daemon HTTP API.
///
/// Only used as the TCP fallback when neither a Unix socket applies nor the
/// `MIMIR_BASE_URL` environment variable / configured `server.bind_addr` is
/// available. See [`base_url`] and `crate::transport::DaemonTransport::resolve`.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

/// Return the TCP fallback base URL of the local Mimir daemon HTTP API.
///
/// Resolution order (first non-blank value wins):
/// 1. `MIMIR_BASE_URL` environment variable.
/// 2. `server.bind_addr` from the Mimir config file (wildcard bind hosts such
///    as `0.0.0.0` are normalised to loopback for the local client).
/// 3. [`DEFAULT_BASE_URL`].
///
/// Used only when no Unix domain socket applies (non-Unix platforms or a
/// remote `MIMIR_BASE_URL`); see `crate::transport::DaemonTransport::resolve`.
///
/// The result is cached via [`LazyLock`] so the environment and config file
/// are read at most once per process. Uses [`Cow`] to avoid allocating on the
/// default path.
#[inline]
pub fn base_url() -> Cow<'static, str> {
    static CACHED: LazyLock<Cow<'static, str>> = LazyLock::new(|| {
        let resolved = config::resolve_base_url(
            std::env::var("MIMIR_BASE_URL").ok().as_deref(),
            config::configured_bind_addr().as_deref(),
        );
        if resolved == DEFAULT_BASE_URL {
            Cow::Borrowed(DEFAULT_BASE_URL)
        } else {
            Cow::Owned(resolved)
        }
    });
    CACHED.clone()
}
