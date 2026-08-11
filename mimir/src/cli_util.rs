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

/// Build a daemon HTTP client for the given base URL.
pub fn make_client(base_url: &str) -> MimirClient {
    MimirClient::new(base_url)
}

/// Pretty-print a JSON value to stdout — the `--json` output mode shared by
/// the `kb` and `connector` command groups.
pub fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("response must serialise to JSON")
    );
}
