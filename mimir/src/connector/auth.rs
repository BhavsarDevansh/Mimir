//! `mimir connector auth` — credential ingest for an existing instance.

use is_terminal::IsTerminal;
use mimir_api_types::IngestTokenRequest;

use super::oauth::{ingest_oauth_bundle, open_in_browser, run_oauth_flow_with_opener};
use super::{
    CredentialKind, credential_kind_for, exit_with_error, make_client, merge_config, print_json,
    prompt_secret, render_client_error, resolve_connector,
};

/// Ingest credentials for an existing connector.
///
/// Re-runnable: completes an instance that was registered without
/// credentials (a non-interactive `add`, or a credential the daemon later
/// rejected), and re-auths after expiry — without `remove` + re-`add`.
/// The daemon's stored config is not exposed on the wire, so the credential
/// kind comes from `--password` / `--token`, an interactive selection when
/// neither is given, or the `auth.kind` of a re-supplied config
/// (`--config-json` / `key=value` pairs). An `auth.kind=oauth` config runs
/// the interactive PKCE loopback flow (A4 / #205) instead of prompting.
pub async fn handle_connector_auth(
    slug: String,
    config: Vec<String>,
    config_json: Option<String>,
    password: Option<String>,
    token: Option<String>,
    json: bool,
    base_url: &str,
) {
    handle_connector_auth_with_opener(
        slug,
        config,
        config_json,
        password,
        token,
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
    token: Option<String>,
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
        if password.is_some() || token.is_some() {
            eprintln!(
                "Warning: --password/--token are ignored for OAuth connectors (the PKCE flow obtains the token)"
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
    // flags, else from an interactive selection.
    let kind = match kind {
        CredentialKind::None => match (password.is_some(), token.is_some()) {
            (true, false) => CredentialKind::AppPassword,
            (false, true) => CredentialKind::ApiToken,
            (true, true) => exit_with_error("pass only one of --password / --token"),
            (false, false) => prompt_credential_kind(&slug),
        },
        kind => kind,
    };
    let secret = match kind {
        CredentialKind::AppPassword => {
            if token.is_some() {
                eprintln!(
                    "Warning: --token given but the connector uses an app password — ignoring it (pass --password instead)"
                );
            }
            password.or_else(|| prompt_secret("App password:"))
        }
        CredentialKind::ApiToken => {
            if password.is_some() {
                eprintln!(
                    "Warning: --password given but the connector uses an API token — ignoring it (pass --token instead)"
                );
            }
            token.or_else(|| prompt_secret("API token:"))
        }
        CredentialKind::OAuth => unreachable!("handled above"),
        CredentialKind::None => unreachable!("resolved above"),
    }
    .unwrap_or_else(|| {
        if !std::io::stdin().is_terminal() {
            exit_with_error(format!(
                "connector '{slug}' needs a credential — run in a terminal, or pass --password / --token"
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
/// with a message pointing at the flags.
fn prompt_credential_kind(slug: &str) -> CredentialKind {
    if !std::io::stdin().is_terminal() {
        exit_with_error(format!(
            "connector '{slug}' needs a credential — run in a terminal, or pass --password / --token"
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
