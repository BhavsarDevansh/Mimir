//! `mimir connector add` — register a connector instance and ingest
//! non-OAuth credentials.

use mimir_api_types::{AddConnectorRequest, IngestTokenRequest};

use super::{
    CredentialKind, add_secret, credential_kind_for, exit_with_error, make_client, merge_config,
    print_json, render_client_error, title_case,
};

/// Register a new connector instance.
///
/// The daemon creates the instance in `Setup` (activation is the `resume`
/// action, A2 / #203). The non-OAuth credential (if the merged config
/// declares one) is prompted for via `inquire` *before* the instance is
/// registered, so a canceled prompt aborts with nothing created; the secret
/// is then ingested through the token route so the instance becomes
/// `authenticated`. A non-interactive run without `--password` / `--token`
/// proceeds with an unauthenticated instance and warns — it can be
/// completed later with `mimir connector auth <slug>`. OAuth configs never
/// prompt here — the interactive PKCE flow is A4 (#205).
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

    // Prompt for the credential before the daemon registers anything: a
    // canceled prompt exits here with no instance created.
    let secret = add_secret(&merged, password, token);

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

    // Non-OAuth credential ingest (secret already resolved pre-create).
    match (credential_kind_for(&merged), secret) {
        (CredentialKind::AppPassword, Some(secret)) => {
            output = client
                .connector_tokens(id, IngestTokenRequest::AppPassword { password: secret })
                .await
                .unwrap_or_else(|e| exit_with_error(ingest_failure(e, &slug)));
        }
        (CredentialKind::ApiToken, Some(secret)) => {
            output = client
                .connector_tokens(id, IngestTokenRequest::ApiToken { token: secret })
                .await
                .unwrap_or_else(|e| exit_with_error(ingest_failure(e, &slug)));
        }
        (kind, None) => {
            if !matches!(kind, CredentialKind::None) {
                eprintln!(
                    "Warning: no credential provided — connector '{slug}' left unauthenticated (pass --password/--token, or run `mimir connector auth {slug}` to complete it later)"
                );
            }
        }
        (CredentialKind::None, Some(_)) => {
            unreachable!("add_secret only returns a secret for non-OAuth kinds")
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

/// Render the exit message when credential ingest fails after the instance
/// was registered: the connector stays unauthenticated and can be completed
/// later with `mimir connector auth <slug>`.
fn ingest_failure(e: mimir_client::ClientError, slug: &str) -> String {
    format!(
        "{} — connector '{slug}' was registered but left unauthenticated; retry with `mimir connector auth {slug}`",
        render_client_error(e)
    )
}
