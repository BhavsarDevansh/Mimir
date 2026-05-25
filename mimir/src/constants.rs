//! Shared constants for the mimir CLI binary.

use std::borrow::Cow;
use std::sync::LazyLock;

/// Default URL for the local Mimir daemon HTTP API.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

/// Return the base URL, preferring `MIMIR_BASE_URL` env override.
///
/// Cached via [`LazyLock`] so the environment is read at most once per
/// process.  Uses [`Cow`] to avoid allocating on the default path.
#[inline]
pub fn base_url() -> Cow<'static, str> {
    static CACHED: LazyLock<Cow<'static, str>> = LazyLock::new(|| {
        std::env::var("MIMIR_BASE_URL")
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(DEFAULT_BASE_URL))
    });
    CACHED.clone()
}
