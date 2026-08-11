//! `mimir connector auth` — credential ingest for an existing instance.

use is_terminal::IsTerminal;
use mimir_api_types::IngestTokenRequest;

use super::{
    CredentialKind, exit_with_error, make_client, print_json, prompt_secret, render_client_error,
    resolve_connector,
};

/// Ingest credentials for an existing connector.
///
/// Re-runnable: completes an instance that was registered without
/// credentials (a non-interactive `add`, or a credential the daemon later
/// rejected), and re-auths after expiry — without `remove` + re-`add`.
/// The daemon's stored config is not exposed on the wire, so the credential
/// kind comes from the `--password` / `--token` flag, or an interactive
/// selection when neither is given.
pub async fn handle_connector_auth(
    slug: String,
    password: Option<String>,
    token: Option<String>,
    json: bool,
    base_url: &str,
) {
    let client = make_client(base_url);
    let conn = resolve_connector(&client, &slug).await;

    let (kind, secret) = match (password, token) {
        (Some(password), None) => (CredentialKind::AppPassword, password),
        (None, Some(token)) => (CredentialKind::ApiToken, token),
        (Some(_), Some(_)) => exit_with_error("pass only one of --password / --token"),
        (None, None) => prompt_credential(&slug),
    };

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
        CredentialKind::None => unreachable!("prompt_credential never selects the None kind"),
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

/// Ask which credential kind the connector uses, then prompt for the
/// secret. Non-terminal stdin aborts with a message pointing at the flags.
fn prompt_credential(slug: &str) -> (CredentialKind, String) {
    if !std::io::stdin().is_terminal() {
        exit_with_error(format!(
            "connector '{slug}' needs a credential — run in a terminal, or pass --password / --token"
        ));
    }
    let kind = match inquire::Select::new(
        "Which credential kind does this connector use?",
        vec!["App password", "API token"],
    )
    .prompt()
    {
        Ok("App password") => CredentialKind::AppPassword,
        Ok("API token") => CredentialKind::ApiToken,
        Ok(_) => unreachable!("select prompt only offers two options"),
        Err(e) => exit_with_error(format!("credential kind prompt failed: {e}")),
    };
    let label = match kind {
        CredentialKind::AppPassword => "App password:",
        CredentialKind::ApiToken => "API token:",
        CredentialKind::None => unreachable!("select prompt only offers two options"),
    };
    let secret =
        prompt_secret(label).unwrap_or_else(|| exit_with_error("credential prompt cancelled"));
    (kind, secret)
}
