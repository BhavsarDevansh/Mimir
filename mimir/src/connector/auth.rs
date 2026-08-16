//! `mimir connector auth` — credential ingest for an existing instance.

use is_terminal::IsTerminal;
use mimir_api_types::IngestTokenRequest;

use super::oauth::{ingest_oauth_bundle, open_in_browser, run_oauth_flow_with_opener};
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
/// The daemon's stored config is not exposed on the wire, so the credential
/// kind comes from `--password` / `--token` / `--password-stdin` /
/// `--token-stdin`, the `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN`
/// env vars (exactly one set), an interactive selection when none is given,
/// or the `auth.kind` of a re-supplied config (`--config-json` / `key=value`
/// pairs). An `auth.kind=oauth` config runs the interactive PKCE loopback
/// flow (A4 / #205) instead of prompting.
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
    base_url: &str,
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
        base_url,
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
    base_url: &str,
    opener: &(dyn Fn(&str) + Send + Sync),
) {
    let client = make_client(base_url);
    let conn = resolve_connector(&client, &slug).await;
    let merged =
        merge_config(&config, config_json.as_deref()).unwrap_or_else(|e| exit_with_error(e));
    let kind = credential_kind_for(&merged);

    // OAuth: the interactive PKCE flow replaces the credential prompt. The
    // re-supplied config only drives the flow; the daemon's stored config
    // remains authoritative for the connector's runtime.
    if matches!(kind, CredentialKind::OAuth) {
        if any_secret_channel(
            password.as_deref(),
            token.as_deref(),
            password_stdin,
            token_stdin,
        ) {
            eprintln!(
                "Warning: --password/--token (or --password-stdin/--token-stdin/MIMIR_CONNECTOR_*) are ignored for OAuth connectors (the PKCE flow obtains the token)"
            );
        }
        let bundle = run_oauth_flow_with_opener(&merged, opener).await;
        let updated = ingest_oauth_bundle(&client, conn.id, &bundle).await;
        if json {
            print_json(&updated);
            return;
        }
        println!(
            "OAuth login complete — credentials stored for connector '{slug}' (auth state: {}). Run `mimir connector resume {slug}` if it is not running.",
            updated.auth_state
        );
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
        CredentialKind::OAuth => unreachable!("handled above"),
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
        vec!["App password", "API token"],
    )
    .prompt()
    {
        Ok("App password") => CredentialKind::AppPassword,
        Ok("API token") => CredentialKind::ApiToken,
        Ok(_) => unreachable!("select prompt only offers two options"),
        Err(e) => exit_with_error(format!("credential kind prompt failed: {e}")),
    }
}
