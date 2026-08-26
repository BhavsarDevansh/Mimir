//! `mimir connector auth` — credential ingest for an existing instance.

use is_terminal::IsTerminal;
use mimir_api_types::{ConnectorAuthConfig, IngestTokenRequest};

use super::oauth::{
    ingest_oauth_bundle, oauth_config_slice, oauth_flow_config_with_secret, open_in_browser,
    run_oauth_flow_with_opener,
};
use super::{
    CredentialKind, ENV_PASSWORD, ENV_TOKEN, any_secret_channel, credential_kind_for, env_secret,
    exit_with_error, make_client, merge_config, print_json, render_client_error, resolve_connector,
    resolve_kind_secret,
};

/// Ingest credentials for an existing connector.
///
/// Re-runnable: completes an instance that was registered without
/// credentials (a non-interactive `add`, or a credential the daemon later
/// rejected), and re-auths after expiry — without `remove` + re-`add`.
/// The credential kind comes from `--password` / `--token` /
/// `--password-stdin` / `--token-stdin`, the `MIMIR_CONNECTOR_PASSWORD` /
/// `MIMIR_CONNECTOR_TOKEN` env vars (exactly one set), an interactive
/// selection when none is given, or the `auth.kind` of a re-supplied config
/// (`--config-json` / `key=value` pairs). An `auth.kind=oauth` config runs
/// the interactive PKCE loopback flow (A4 / #205) instead of prompting; when
/// no kind is re-supplied, a connector whose stored config uses OAuth re-runs
/// the PKCE flow from the daemon-exposed non-secret auth config, so re-auth
/// after expiry works without re-supplying the endpoints (issue #507).
#[allow(clippy::too_many_arguments)] // mirrors the clap field count (kb style)
pub async fn handle_connector_auth(
    slug: String,
    config: Vec<String>,
    config_json: Option<String>,
    password: Option<String>,
    password_stdin: bool,
    token: Option<String>,
    token_stdin: bool,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    handle_connector_auth_with_opener(
        slug,
        config,
        config_json,
        password,
        password_stdin,
        token,
        token_stdin,
        json,
        transport,
        &open_in_browser,
    )
    .await;
}

/// Testable core of [`handle_connector_auth`]: `opener` is called with the
/// authorize URL when the config declares an OAuth auth method.
#[allow(clippy::too_many_arguments)] // mirrors the clap field count (kb style)
pub(crate) async fn handle_connector_auth_with_opener(
    slug: String,
    config: Vec<String>,
    config_json: Option<String>,
    password: Option<String>,
    password_stdin: bool,
    token: Option<String>,
    token_stdin: bool,
    json: bool,
    transport: &crate::transport::DaemonTransport,
    opener: &(dyn Fn(&str) + Send + Sync),
) {
    let client = make_client(transport);
    let conn = resolve_connector(&client, &slug).await;
    let merged =
        merge_config(&config, config_json.as_deref()).unwrap_or_else(|e| exit_with_error(e));
    let mut kind = credential_kind_for(&merged);

    // Stored-config fallback (issue #507): the daemon surfaces the stored
    // non-secret auth config, so an OAuth connector can re-auth without the
    // user re-supplying `auth.kind=oauth auth.auth_uri=...` — the PKCE flow
    // runs from the stored endpoints instead. A re-supplied `auth.kind` (any
    // kind) keeps precedence: it is the user's explicit intent.
    let stored_oauth = if matches!(kind, CredentialKind::None) {
        conn.auth
            .as_ref()
            .filter(|auth| auth.kind == "oauth")
            .map(stored_oauth_config)
    } else {
        None
    };
    if stored_oauth.is_some() {
        kind = CredentialKind::OAuth;
    }

    // OAuth: the interactive PKCE flow replaces the credential prompt. The
    // driving config is the re-supplied one when it declares OAuth, else the
    // stored non-secret config with any re-supplied OAuth fields overlaid
    // (issue #511 review: a confidential client that re-supplies only
    // `auth.client_secret` keeps the stored endpoints / client id / scopes
    // while the secret reaches the PKCE exchange), else (interactive "OAuth
    // 2.0" selection) the re-supplied config which must then carry the
    // endpoints.
    let oauth_config = if credential_kind_for(&merged) == CredentialKind::OAuth {
        merged.clone()
    } else if let Some(stored) = &stored_oauth {
        overlay_oauth_fields(stored, &merged)
    } else {
        merged.clone()
    };
    if matches!(kind, CredentialKind::OAuth) {
        reauth_oauth(
            &slug,
            conn.id,
            &oauth_config,
            password.as_deref(),
            token.as_deref(),
            password_stdin,
            token_stdin,
            json,
            &client,
            opener,
        )
        .await;
        return;
    }

    // Non-OAuth: the kind comes from the config when declared, else from the
    // flags / stdin flags, else from the MIMIR_CONNECTOR_* env vars, else
    // from an interactive selection.
    let kind = match kind {
        CredentialKind::None => {
            if (password.is_some() || password_stdin) && (token.is_some() || token_stdin) {
                exit_with_error(
                    "pass only one of --password / --token / --password-stdin / --token-stdin",
                );
            }
            if password.is_some() || password_stdin {
                CredentialKind::AppPassword
            } else if token.is_some() || token_stdin {
                CredentialKind::ApiToken
            } else {
                let env_password = env_secret(ENV_PASSWORD);
                let env_token = env_secret(ENV_TOKEN);
                match (env_password.is_some(), env_token.is_some()) {
                    (true, true) => exit_with_error(
                        "set only one of MIMIR_CONNECTOR_PASSWORD / MIMIR_CONNECTOR_TOKEN",
                    ),
                    (true, false) => CredentialKind::AppPassword,
                    (false, true) => CredentialKind::ApiToken,
                    (false, false) => prompt_credential_kind(&slug),
                }
            }
        }
        kind => kind,
    };
    // The interactive "OAuth 2.0" selection routes back into the shared PKCE
    // flow. The driving config is the re-supplied one (which must carry the
    // endpoints — the stored config did not declare OAuth, or the flow above
    // would already have run); the guidance error names the missing fields.
    if matches!(kind, CredentialKind::OAuth) {
        reauth_oauth(
            &slug,
            conn.id,
            &oauth_config,
            password.as_deref(),
            token.as_deref(),
            password_stdin,
            token_stdin,
            json,
            &client,
            opener,
        )
        .await;
        return;
    }

    let secret = resolve_kind_secret(kind, password, token, password_stdin, token_stdin)
    .unwrap_or_else(|| {
        if !std::io::stdin().is_terminal() {
            exit_with_error(format!(
                "connector '{slug}' needs a credential — run in a terminal, or pass --password / --token, set MIMIR_CONNECTOR_PASSWORD / MIMIR_CONNECTOR_TOKEN, or pipe the secret via --password-stdin / --token-stdin"
            ));
        }
        exit_with_error("credential prompt cancelled")
    });

    let updated = match kind {
        CredentialKind::AppPassword => client
            .connector_tokens(
                conn.id,
                IngestTokenRequest::AppPassword { password: secret },
            )
            .await
            .unwrap_or_else(|e| exit_with_error(render_client_error(e))),
        CredentialKind::ApiToken => client
            .connector_tokens(conn.id, IngestTokenRequest::ApiToken { token: secret })
            .await
            .unwrap_or_else(|e| exit_with_error(render_client_error(e))),
        CredentialKind::OAuth => unreachable!("routed into the PKCE flow above"),
        CredentialKind::None => unreachable!("resolved above"),
    };

    if json {
        print_json(&updated);
        return;
    }
    println!(
        "Credentials stored for connector '{slug}' (auth state: {}). Run `mimir connector resume {slug}` if it is not running.",
        updated.auth_state
    );
}

/// Run the shared interactive PKCE flow for a re-auth and ingest the
/// resulting bundle. `oauth_config` carries the OAuth endpoints (re-supplied
/// config, the stored non-secret config, or the interactive fallback);
/// missing fields exit with the exact config keys to supply (issue #507).
#[allow(clippy::too_many_arguments)] // mirrors the caller's clap field count
async fn reauth_oauth(
    slug: &str,
    connector_id: i32,
    oauth_config: &serde_json::Value,
    password: Option<&str>,
    token: Option<&str>,
    password_stdin: bool,
    token_stdin: bool,
    json: bool,
    client: &mimir_client::MimirClient,
    opener: &(dyn Fn(&str) + Send + Sync),
) {
    if any_secret_channel(password, token, password_stdin, token_stdin) {
        eprintln!(
            "Warning: --password/--token (or --password-stdin/--token-stdin/MIMIR_CONNECTOR_*) are ignored for OAuth connectors (the PKCE flow obtains the token)"
        );
    }
    if let Err(reason) = oauth_flow_config_with_secret(oauth_config, None) {
        exit_with_error(format!(
            "cannot run the OAuth login for connector '{slug}': {reason} — re-run supplying the OAuth fields, e.g. `mimir connector auth {slug} auth.kind=oauth auth.auth_uri=... auth.token_endpoint=... auth.client_id=...` (confidential clients also pass auth.client_secret=...)"
        ));
    }
    let bundle = run_oauth_flow_with_opener(oauth_config, opener).await;
    let updated = ingest_oauth_bundle(
        client,
        connector_id,
        &bundle,
        oauth_config_slice(oauth_config),
    )
    .await;
    if json {
        print_json(&updated);
        return;
    }
    println!(
        "OAuth login complete — credentials stored for connector '{slug}' (auth state: {}). Run `mimir connector resume {slug}` if it is not running.",
        updated.auth_state
    );
}

/// Rebuild a `{"auth": {...}}` config value from the daemon's non-secret auth
/// slice so the shared PKCE flow helpers read the stored OAuth endpoints
/// without the user re-supplying them (issue #507). `client_secret` is never
/// on the wire (it lives in the credential bundle), so the rebuilt config
/// omits it — the flow still works for the PKCE public clients Mimir targets,
/// and the stored bundle's secret continues to serve refreshes.
fn stored_oauth_config(auth: &ConnectorAuthConfig) -> serde_json::Value {
    let mut oauth = serde_json::Map::new();
    oauth.insert("kind".to_string(), serde_json::json!("oauth"));
    if let Some(username) = &auth.username {
        oauth.insert("username".to_string(), serde_json::json!(username));
    }
    if let Some(auth_uri) = &auth.auth_uri {
        oauth.insert("auth_uri".to_string(), serde_json::json!(auth_uri));
    }
    if let Some(token_endpoint) = &auth.token_endpoint {
        oauth.insert(
            "token_endpoint".to_string(),
            serde_json::json!(token_endpoint),
        );
    }
    if let Some(client_id) = &auth.client_id {
        oauth.insert("client_id".to_string(), serde_json::json!(client_id));
    }
    if let Some(scopes) = &auth.scopes {
        oauth.insert("scopes".to_string(), serde_json::json!(scopes));
    }
    serde_json::json!({ "auth": oauth })
}

/// Overlay re-supplied OAuth `auth` fields onto the stored non-secret OAuth
/// config (issue #511 review). The stored slice is the base — endpoints,
/// client id, and scopes survive a kind-less re-supply such as
/// `auth.client_secret=...` — while every re-supplied field (notably the
/// secret, which lives in the credential bundle, never `config_json`) takes
/// precedence. `kind` is never overlaid: the stored slice already declares
/// `oauth`, and the fallback path only runs when the re-supplied config
/// declares no recognized kind.
fn overlay_oauth_fields(
    stored: &serde_json::Value,
    supplied: &serde_json::Value,
) -> serde_json::Value {
    let mut auth = stored
        .pointer("/auth")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    if let Some(supplied_auth) = supplied.pointer("/auth").and_then(|v| v.as_object()) {
        for (key, value) in supplied_auth {
            if key != "kind" {
                auth.insert(key.clone(), value.clone());
            }
        }
    }
    serde_json::json!({ "auth": auth })
}

/// Ask which credential kind the connector uses. Non-terminal stdin aborts
/// with a message pointing at the non-visible channels.
fn prompt_credential_kind(slug: &str) -> CredentialKind {
    if !std::io::stdin().is_terminal() {
        exit_with_error(format!(
            "connector '{slug}' needs a credential — run in a terminal, or pass --password / --token, set MIMIR_CONNECTOR_PASSWORD / MIMIR_CONNECTOR_TOKEN, or pipe the secret via --password-stdin / --token-stdin"
        ));
    }
    match inquire::Select::new(
        "Which credential kind does this connector use?",
        vec!["App password", "API token", "OAuth 2.0"],
    )
    .prompt()
    {
        Ok("App password") => CredentialKind::AppPassword,
        Ok("API token") => CredentialKind::ApiToken,
        Ok("OAuth 2.0") => CredentialKind::OAuth,
        Ok(_) => unreachable!("select prompt only offers three options"),
        Err(e) => exit_with_error(format!("credential kind prompt failed: {e}")),
    }
}
