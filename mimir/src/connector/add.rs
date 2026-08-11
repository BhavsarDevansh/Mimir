//! `mimir connector add` — register a connector instance and ingest
//! non-OAuth credentials.

use mimir_api_types::{AddConnectorRequest, IngestTokenRequest};

use super::{
    CredentialKind, credential_kind_for, exit_with_error, make_client, merge_config, print_json,
    prompt_secret, render_client_error, title_case,
};

/// Register a new connector instance.
///
/// The daemon creates the instance in `Setup` (activation is the `resume`
/// action, A2 / #203). When the merged config declares a non-OAuth `auth`
/// kind, the matching credential is prompted for via `inquire` (or taken
/// from `--password` / `--token`) and ingested through the token route so
/// the instance becomes `authenticated`. OAuth configs never prompt here —
/// the interactive PKCE flow is A4 (#205).
#[allow(clippy::too_many_arguments)] // mirrors the clap field count (kb style)
pub async fn handle_connector_add(
    connector_type: String,
    backend: String,
    config: Vec<String>,
    config_json: Option<String>,
    slug: Option<String>,
    name: Option<String>,
    password: Option<String>,
    token: Option<String>,
    json: bool,
    base_url: &str,
) {
    let client = make_client(base_url);
    let merged =
        merge_config(&config, config_json.as_deref()).unwrap_or_else(|e| exit_with_error(e));
    let slug = slug.unwrap_or_else(|| connector_type.to_ascii_lowercase());
    let display_name = name.unwrap_or_else(|| title_case(&connector_type));

    let request = AddConnectorRequest {
        connector_type,
        backend,
        slug: slug.clone(),
        display_name,
        config_json: merged.clone(),
    };
    let created = client
        .connector_add(request)
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    let id = created.id;
    let mut output = created;

    // Non-OAuth credential ingest. The prompt is skipped when a flag was
    // supplied or stdin is not a terminal (scripts must pass the flag).
    match credential_kind_for(&merged) {
        CredentialKind::AppPassword => {
            if token.is_some() {
                eprintln!(
                    "Warning: --token given but auth.kind is 'app_password' — ignoring it (pass --password instead)"
                );
            }
            match password.or_else(|| prompt_secret("App password:")) {
                Some(secret) => {
                    output = client
                        .connector_tokens(id, IngestTokenRequest::AppPassword { password: secret })
                        .await
                        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
                }
                None => eprintln!(
                    "Warning: no app password provided — connector '{slug}' left unauthenticated (pass --password to supply one non-interactively)"
                ),
            }
        }
        CredentialKind::ApiToken => {
            if password.is_some() {
                eprintln!(
                    "Warning: --password given but auth.kind is 'api_token' — ignoring it (pass --token instead)"
                );
            }
            match token.or_else(|| prompt_secret("API token:")) {
                Some(secret) => {
                    output = client
                        .connector_tokens(id, IngestTokenRequest::ApiToken { token: secret })
                        .await
                        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
                }
                None => eprintln!(
                    "Warning: no API token provided — connector '{slug}' left unauthenticated (pass --token to supply one non-interactively)"
                ),
            }
        }
        CredentialKind::None => {
            if password.is_some() || token.is_some() {
                eprintln!(
                    "Warning: --password/--token given but config declares no non-OAuth credential kind — ignoring them (set auth.kind=app_password or auth.kind=api_token)"
                );
            }
        }
    }

    if json {
        print_json(&output);
        return;
    }
    println!(
        "Added connector '{slug}' ({} / {}, id {}, status {}, auth {}).",
        output.connector_type, output.backend, output.id, output.status, output.auth_state
    );
    println!(
        "Next: run `mimir connector resume {slug}` to activate it, then `mimir connector sync {slug}` to sync."
    );
}
