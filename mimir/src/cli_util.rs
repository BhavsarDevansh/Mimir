//! Shared helpers for the `mimir` CLI binary.
//!
//! `exit_with_error`, `make_client`, and `print_json` are used by every
//! command group that talks to the daemon (`kb`, `connector`). Keeping them
//! here instead of redefining them per module avoids duplication (DRY).

use mimir_client::MimirClient;

/// Print an error to stderr and exit with a non-zero status.
pub fn exit_with_error(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {}", msg);
    std::process::exit(1);
}

/// Build a daemon HTTP client for the given base URL, authenticating with the
/// local API token (issue #281). The token is loaded — or generated — from
/// the data dir, so the CLI works unmodified after `mimir init` and can even
/// create the token before auto-starting the daemon.
pub fn make_client(base_url: &str) -> MimirClient {
    match mimir_core::auth::load_or_create_api_token() {
        Ok(token) => MimirClient::with_token(base_url, token),
        Err(error) => {
            eprintln!(
                "Warning: failed to load the API token ({error}); requests may be rejected by the daemon."
            );
            MimirClient::new(base_url)
        }
    }
}

/// Pretty-print a JSON value to stdout — the `--json` output mode shared by
/// the `kb` and `connector` command groups.
pub fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("response must serialise to JSON")
    );
}
