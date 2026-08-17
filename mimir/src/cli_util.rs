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
/// create the token before auto-starting the daemon. If the token cannot be
/// loaded or attached as a header, a warning is printed and a tokenless
/// client is returned so the daemon's `401` surfaces the problem instead of
/// a panic. A malformed base URL still panics via [`MimirClient::new`],
/// matching the historical CLI behaviour.
pub fn make_client(base_url: &str) -> MimirClient {
    let token = match mimir_core::auth::load_or_create_api_token() {
        Ok(token) => token,
        Err(error) => {
            eprintln!(
                "Warning: failed to load the API token ({error}); requests may be rejected by the daemon."
            );
            return MimirClient::new(base_url);
        }
    };
    client_with_token(base_url, token)
}

/// Build a token-bearing client, falling back to a tokenless client with a
/// warning when the token cannot be attached as a header (e.g. it contains
/// characters that are invalid in a header value).
fn client_with_token(base_url: &str, token: String) -> MimirClient {
    match MimirClient::try_new_with_token(
        base_url,
        token,
        MimirClient::DEFAULT_CONNECT_TIMEOUT,
        MimirClient::DEFAULT_TOTAL_TIMEOUT,
    ) {
        Ok(client) => client,
        Err(error) => {
            eprintln!(
                "Warning: failed to attach the API token ({error}); requests may be rejected by the daemon."
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

#[cfg(test)]
mod tests {
    use super::client_with_token;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header_exists, method, path},
    };

    #[tokio::test]
    async fn client_with_token_falls_back_to_tokenless_client_when_token_is_invalid() {
        let server = MockServer::start().await;
        // Mounted first so the tokenless mock below wins for requests without
        // an Authorization header; a token-bearing request matches both and the
        // more recently mounted 401 mock takes precedence.
        Mock::given(method("GET"))
            .and(path("/memory"))
            .respond_with(ResponseTemplate::new(200).set_body_string("memory contents"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/memory"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        // A newline is invalid in a header value, so the token cannot be attached.
        let client = client_with_token(&server.uri(), "bad\nheader".to_string());
        let memory = client.memory().await.unwrap();

        assert_eq!(memory, "memory contents");
    }
}
