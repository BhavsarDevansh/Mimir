//! `mimir connector add` — register a connector instance and ingest
//! credentials.

use mimir_api_types::{AddConnectorRequest, IngestTokenRequest};
use mimir_client::MimirClient;

use super::oauth::{ingest_oauth_bundle, open_in_browser, run_oauth_flow_with_opener};
use super::{
    CredentialKind, add_secret, credential_kind_for, exit_with_error, make_client, merge_config,
    print_json, render_client_error, title_case,
};

/// Register a new connector instance.
///
/// The daemon creates the instance in `Setup` (activation is the `resume`
/// action, A2 / #203). The credential is acquired *before* the instance is
/// registered, so a canceled prompt or aborted OAuth flow exits with nothing
/// created: non-OAuth kinds resolve the secret per kind with the precedence
/// flag → stdin flag (`--password-stdin` / `--token-stdin`) →
/// `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN` env var → `inquire`
/// prompt (issue #270), while `auth.kind=oauth` configs run the interactive
/// PKCE loopback flow (A4 / #205) and POST the exchanged
/// `SecretBundle::OAuth` to the token route so the instance becomes
/// `authenticated`. A non-interactive run without any channel proceeds with
/// an unauthenticated instance and warns — it can be completed later with
/// `mimir connector auth <slug>`.
#[allow(clippy::too_many_arguments)] // mirrors the clap field count (kb style)
pub async fn handle_connector_add(
    connector_type: String,
    backend: String,
    config: Vec<String>,
    config_json: Option<String>,
    slug: Option<String>,
    name: Option<String>,
    password: Option<String>,
    password_stdin: bool,
    token: Option<String>,
    token_stdin: bool,
    json: bool,
    base_url: &str,
) {
    handle_connector_add_with_opener(
        connector_type,
        backend,
        config,
        config_json,
        slug,
        name,
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

/// Testable core of [`handle_connector_add`]: `opener` is called with the
/// authorize URL when the config declares an OAuth auth method (tests inject
/// a fake opener that drives the loopback callback).
#[allow(clippy::too_many_arguments)] // mirrors the clap field count (kb style)
pub(crate) async fn handle_connector_add_with_opener(
    connector_type: String,
    backend: String,
    config: Vec<String>,
    config_json: Option<String>,
    slug: Option<String>,
    name: Option<String>,
    password: Option<String>,
    password_stdin: bool,
    token: Option<String>,
    token_stdin: bool,
    json: bool,
    base_url: &str,
    opener: &(dyn Fn(&str) + Send + Sync),
) {
    let client = make_client(base_url);
    validate_backend(&client, &connector_type, &backend).await;
    let merged =
        merge_config(&config, config_json.as_deref()).unwrap_or_else(|e| exit_with_error(e));
    let slug = slug.unwrap_or_else(|| connector_type.to_ascii_lowercase());
    let display_name = name.unwrap_or_else(|| title_case(&connector_type));
    let kind = credential_kind_for(&merged);

    // Acquire the credential before the daemon registers anything: a
    // canceled prompt or aborted OAuth flow exits here with no instance
    // created.
    let oauth_bundle = if matches!(kind, CredentialKind::OAuth) {
        Some(run_oauth_flow_with_opener(&merged, opener).await)
    } else {
        None
    };
    let secret = add_secret(&merged, password, token, password_stdin, token_stdin);

    let output = register_and_ingest(
        &client,
        connector_type,
        backend,
        slug.clone(),
        display_name,
        merged,
        kind,
        oauth_bundle,
        secret,
    )
    .await;
    if !json {
        println!(
            "Added connector '{}' ({} / {}, id {}, status {}, mode {}, auth {}).",
            output.slug,
            output.connector_type,
            output.backend,
            output.id,
            output.status,
            output.mode.as_deref().unwrap_or("-"),
            output.auth_state
        );
        match output.mode.as_deref() {
            Some("push") => println!(
                "Next: run `mimir connector resume {slug}` to activate it — it then syncs automatically (push) and manual sync is not supported."
            ),
            Some("polling") => println!(
                "Next: run `mimir connector resume {slug}` to activate it, then `mimir connector sync {slug}` to sync."
            ),
            // `auto` email mode resolves to push or polling only after the
            // first capability probe; until then the mode is unknown, so do
            // not claim manual sync works or that it is unsupported (issue
            // #397 review).
            _ => println!(
                "Next: run `mimir connector resume {slug}` to activate it — its sync mode (push vs polling) is detected on the first connect."
            ),
        }
    } else {
        print_json(&output);
    }
}

/// Register the connector instance and ingest its credential, returning the
/// updated [`ConnectorResponse`]. Shared by the flag-based `add` flow and the
/// interactive wizard: the caller resolves the
/// `(type, backend, name, slug, config)` and the credential (OAuth bundle or
/// secret) *before* anything is registered, so a canceled prompt or aborted
/// OAuth flow exits with nothing created. The caller owns the summary output
/// (the wizard auto-activates and reports the active state, issue #397).
///
/// The daemon creates the instance in `Setup` (activation is the `resume`
/// action, A2 / #203); credential ingest flips it to `authenticated`.
#[allow(clippy::too_many_arguments)] // one argument per resolved add-flow input (flag form and wizard share it)
pub(crate) async fn register_and_ingest(
    client: &mimir_client::MimirClient,
    connector_type: String,
    backend: String,
    slug: String,
    display_name: String,
    config_json: serde_json::Value,
    kind: CredentialKind,
    oauth_bundle: Option<mimir_connectors::SecretBundle>,
    secret: Option<String>,
) -> mimir_api_types::ConnectorResponse {
    let request = AddConnectorRequest {
        connector_type,
        backend,
        slug: slug.clone(),
        display_name,
        config_json,
    };
    let created = client
        .connector_add(request)
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    let id = created.id;
    let mut output = created;

    // Credential ingest (secret already resolved pre-create).
    match (kind, oauth_bundle, secret) {
        (CredentialKind::OAuth, Some(bundle), _) => {
            output = ingest_oauth_bundle(client, id, &bundle).await;
        }
        (CredentialKind::AppPassword, None, Some(secret)) => {
            output = client
                .connector_tokens(id, IngestTokenRequest::AppPassword { password: secret })
                .await
                .unwrap_or_else(|e| exit_with_error(ingest_failure(e, &slug)));
        }
        (CredentialKind::ApiToken, None, Some(secret)) => {
            output = client
                .connector_tokens(id, IngestTokenRequest::ApiToken { token: secret })
                .await
                .unwrap_or_else(|e| exit_with_error(ingest_failure(e, &slug)));
        }
        (kind, None, None) => {
            if !matches!(kind, CredentialKind::None) {
                eprintln!(
                    "Warning: no credential provided — connector '{slug}' left unauthenticated (pass --password/--token, set MIMIR_CONNECTOR_PASSWORD/MIMIR_CONNECTOR_TOKEN, pipe via --password-stdin/--token-stdin, or run `mimir connector auth {slug}` to complete it later)"
                );
            }
        }
        _ => unreachable!(
            "oauth_bundle is only Some for OAuth kinds and add_secret only returns a secret for non-OAuth kinds"
        ),
    }

    output
}

/// Pre-flight check against the daemon's catalog so a typo'd
/// `(connector_type, backend)` pair fails before the credential prompt or
/// OAuth flow (issue #271). The daemon remains authoritative — `POST
/// /connectors` still validates the pair — so this is a UX fast-fail, not a
/// security boundary.
async fn validate_backend(client: &MimirClient, connector_type: &str, backend: &str) {
    let catalog = client
        .connector_catalog()
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    let normalized_type = connector_type.to_ascii_lowercase();
    if catalog
        .entries
        .iter()
        .any(|entry| entry.connector_type == normalized_type && entry.backend == backend)
    {
        return;
    }
    let type_backends = catalog
        .entries
        .iter()
        .filter(|entry| entry.connector_type == normalized_type)
        .map(|entry| entry.backend.as_str())
        .collect::<Vec<_>>();
    let message = if type_backends.is_empty() {
        if catalog.entries.is_empty() {
            exit_with_error(no_backends_message());
        }
        let supported = catalog
            .entries
            .iter()
            .map(|entry| format!("{}/{}", entry.connector_type, entry.backend))
            .collect::<Vec<_>>()
            .join(", ");
        format!("unknown connector type '{connector_type}'; supported pairs: {supported}")
    } else {
        format!(
            "connector type '{connector_type}' does not support backend '{backend}'; supported backends: {}",
            type_backends.join(", ")
        )
    };
    exit_with_error(message);
}

/// Shared message when the daemon has no connector backends registered
/// (used by the flag form and the interactive wizard — DRY).
pub(crate) fn no_backends_message() -> &'static str {
    "the daemon has no connector backends registered (check its build features)"
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
