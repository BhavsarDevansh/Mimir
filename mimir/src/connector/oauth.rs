//! Shared OAuth PKCE flow helpers for the `connector` command group
//! (Phase 3 A4 / issue #205).
//!
//! The interactive PKCE login runs entirely in the CLI process: the flow
//! (`mimir_connectors::oauth::pkce`) binds an ephemeral loopback listener,
//! opens the provider's authorize URL in the browser, receives the redirect,
//! and exchanges the code. The resulting [`SecretBundle::OAuth`] is POSTed to
//! the daemon's token-ingest route (`POST /connectors/{id}/tokens`, A2 /
//! #203) — the daemon never runs a transient HTTP server.

use mimir_api_types::IngestTokenRequest;
use mimir_connectors::SecretBundle;
use mimir_connectors::oauth::{
    DEFAULT_FLOW_TIMEOUT, OAuthHttpClient, PkceFlowConfig, run_pkce_flow,
};

use super::exit_with_error;

/// Extract the OAuth client configuration the PKCE flow needs from a merged
/// connector config (`auth.kind=oauth`), failing on missing fields. URL
/// validity is checked by the flow itself (`AuthUrl` / the shared token
/// endpoint gate) before any browser is opened, so it is not duplicated
/// here.
pub(crate) fn oauth_flow_config(config: &serde_json::Value) -> Result<PkceFlowConfig, String> {
    let auth = config
        .pointer("/auth")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "OAuth config is missing the `auth` object".to_string())?;
    let required = |key: &str| -> Result<String, String> {
        auth.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("OAuth config is missing `auth.{key}`"))
    };
    let client_secret = auth
        .get("client_secret")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let scopes = auth.get("scopes").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect()
    });
    Ok(PkceFlowConfig {
        auth_uri: required("auth_uri")?,
        token_endpoint: required("token_endpoint")?,
        client_id: required("client_id")?,
        client_secret,
        scopes,
    })
}

/// Open the authorize URL in the user's default browser, printing it first
/// so a headless/SSH session can complete the login manually. A browser-open
/// failure is non-fatal — the printed URL is the fallback. Progress goes to
/// stderr so `--json` command output on stdout stays valid JSON.
pub(crate) fn open_in_browser(url: &str) {
    eprintln!("If the browser does not open automatically, visit:\n  {url}");
    if let Err(e) = webbrowser::open(url) {
        eprintln!("Could not open a browser automatically ({e}) — open the URL above manually.");
    }
}

/// Run the interactive PKCE flow for an OAuth config. `opener` is called
/// with the authorize URL — the default [`open_in_browser`] opens the user's
/// browser; tests inject a fake opener that drives the loopback callback.
/// Exits with a clear message on any failure.
pub(crate) async fn run_oauth_flow_with_opener(
    config: &serde_json::Value,
    opener: &(dyn Fn(&str) + Send + Sync),
) -> SecretBundle {
    let flow_config = oauth_flow_config(config).unwrap_or_else(|e| exit_with_error(e));
    let http = OAuthHttpClient::new().unwrap_or_else(|e| exit_with_error(e.to_string()));
    eprintln!("Starting OAuth login — complete the authorization in the browser that opens.");
    run_pkce_flow(&flow_config, &http, opener, DEFAULT_FLOW_TIMEOUT)
        .await
        .unwrap_or_else(|e| exit_with_error(e))
}

/// Convert an exchanged [`SecretBundle`] into the daemon's wire request
/// (`IngestTokenRequest::OAuth`), serialising the expiry as RFC-3339.
pub(crate) fn oauth_ingest_request(bundle: &SecretBundle) -> IngestTokenRequest {
    let SecretBundle::OAuth {
        access_token,
        refresh_token,
        expires_at,
    } = bundle
    else {
        unreachable!("the PKCE flow always returns an OAuth bundle")
    };
    IngestTokenRequest::OAuth {
        access_token: access_token.clone(),
        refresh_token: refresh_token.clone(),
        expires_at: expires_at.map(|dt| dt.to_rfc3339()),
    }
}

/// Ingest an OAuth bundle for a connector, exiting with a clear message on
/// failure. Shared by `add` and `auth`.
pub(crate) async fn ingest_oauth_bundle(
    client: &mimir_client::MimirClient,
    id: i32,
    bundle: &SecretBundle,
) -> mimir_api_types::ConnectorResponse {
    client
        .connector_tokens(id, oauth_ingest_request(bundle))
        .await
        .unwrap_or_else(|e| exit_with_error(super::render_client_error(e)))
}
