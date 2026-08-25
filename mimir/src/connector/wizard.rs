//! `mimir connector add` interactive wizard — guided connector setup with no
//! flags required.
//!
//! Running `mimir connector add` with no arguments walks the user through:
//! type/backend selection (from the daemon's live catalog), display name
//! (defaults to the type), slug (defaults to the slugified name), per-backend
//! configuration, and authentication — OAuth runs the shared PKCE loopback
//! flow (A4 / #205) which prints the authorize URL and opens the browser, app
//! passwords and OAuth client secrets are prompted hidden, and tokens are
//! never echoed. Credentials — including the OAuth client secret — are
//! stored by the daemon's [`SecretStore`](mimir_connectors::secrets)
//! (per-slug `0600` files), never in the config.
///
/// The wizard is purely a UX layer over the same daemon routes as the flag
/// form: it pre-flights the catalog, builds `config_json` the same way
/// `key=value` pairs do, and registers via the shared
/// [`register_and_ingest`](super::add::register_and_ingest) core.
use is_terminal::IsTerminal;
use serde_json::{Value, json};

use super::add::no_backends_message;
use super::add::register_and_ingest;
use super::oauth::{open_in_browser, run_oauth_flow_with_opener_and_secret};
use super::{
    CredentialKind, exit_with_error, make_client, print_json, render_client_error, title_case,
};

/// Google OAuth authorize endpoint used as the Gmail IMAP default (the
/// user's Google Cloud OAuth client points here).
const GOOGLE_AUTH_URI: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Google OAuth token endpoint (RFC 6749 code exchange + refresh).
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// Gmail IMAP (XOAUTH2) requires the full mailbox scope; there is no
/// read-only IMAP scope. The connector itself only reads (write-back is a
/// separate, explicit `act` dispatch and is not implemented for email).
const GMAIL_SCOPE: &str = "https://mail.google.com/";
/// Default CalDAV OAuth scope for Google Calendar (the calendar connector's
/// documented OAuth provider). The wizard pre-fills it so the authorize URL
/// always carries a `scope` parameter.
const CALDAV_SCOPE: &str = "https://www.googleapis.com/auth/calendar";

/// Microsoft identity platform authorize endpoints by account audience
/// (issue #467): `/common/` only works for app registrations whose
/// "Supported account types" is "Accounts in any organizational directory
/// and personal Microsoft accounts" (the "All" audience); personal-only
/// registrations must use `/consumers/`, multitenant org-only ones
/// `/organizations/`, and single-tenant ("this organizational directory
/// only") ones embed the tenant ID or domain in the path. The Outlook
/// preset asks which audience applies and pre-fills the matching endpoints
/// (the user still brings their own app registration: Mimir has no public
/// client ID).
const MICROSOFT_AUTH_URI_COMMON: &str =
    "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const MICROSOFT_AUTH_URI_CONSUMERS: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const MICROSOFT_AUTH_URI_ORGANIZATIONS: &str =
    "https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize";
/// Microsoft identity platform token endpoints, matching the authorize
/// endpoints above per audience.
const MICROSOFT_TOKEN_ENDPOINT_COMMON: &str =
    "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const MICROSOFT_TOKEN_ENDPOINT_CONSUMERS: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const MICROSOFT_TOKEN_ENDPOINT_ORGANIZATIONS: &str =
    "https://login.microsoftonline.com/organizations/oauth2/v2.0/token";
/// Microsoft identity platform account audiences offered by the Outlook
/// / Office 365 preset (issue #467): the authorize/token endpoints differ
/// per audience, and a mismatch with the app registration's "Supported
/// account types" fails the authorize request, so the wizard asks which
/// applies and pre-fills the matching tenant.
#[derive(Clone, Copy)]
enum MicrosoftAccountType {
    /// Personal Microsoft accounts (Outlook.com / Hotmail) — `/consumers/`.
    Consumers,
    /// Work or school accounts (Entra ID / Office 365) — `/organizations/`.
    Organizations,
    /// Any Microsoft account ("All" audience) — `/common/`.
    Common,
    /// Work or school accounts in this organizational directory only
    /// (single-tenant app registration): the endpoints embed the tenant ID
    /// or domain, so the wizard asks for it and builds both endpoints from
    /// it.
    SingleTenant,
}

/// Wizard choices for [`MicrosoftAccountType`] (issue #467), in the order
/// offered by the prompt: the picked index reads straight out of this
/// array (via [`MicrosoftAccountType::from_index`]) and the labels live on
/// the variants themselves, so the option order and the endpoint mapping
/// cannot drift.
const MICROSOFT_ACCOUNT_TYPE_OPTIONS: [MicrosoftAccountType; 4] = [
    MicrosoftAccountType::Consumers,
    MicrosoftAccountType::Organizations,
    MicrosoftAccountType::Common,
    MicrosoftAccountType::SingleTenant,
];

impl MicrosoftAccountType {
    /// Prompt label for this audience.
    fn label(self) -> &'static str {
        match self {
            Self::Consumers => "Personal Microsoft account (Outlook.com / Hotmail)",
            Self::Organizations => {
                "Work or school account (any organizational directory — multitenant)"
            }
            Self::Common => "Any Microsoft account — app registration allows both (All audience)",
            Self::SingleTenant => {
                "Work or school account (this organizational directory only — single-tenant)"
            }
        }
    }

    /// Map the picked wizard option index onto the audience via
    /// [`MICROSOFT_ACCOUNT_TYPE_OPTIONS`]; indices outside the list fall
    /// back to [`Self::Common`] so a driver can never produce an unknown
    /// tenant.
    fn from_index(index: usize) -> Self {
        MICROSOFT_ACCOUNT_TYPE_OPTIONS
            .get(index)
            .copied()
            .unwrap_or(Self::Common)
    }

    /// The fixed authorize/token endpoint pair for this audience; `None`
    /// for [`Self::SingleTenant`], whose endpoints embed the tenant ID or
    /// domain and must be built from it (see
    /// [`microsoft_tenant_endpoints`]).
    fn fixed_endpoints(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Consumers => Some((
                MICROSOFT_AUTH_URI_CONSUMERS,
                MICROSOFT_TOKEN_ENDPOINT_CONSUMERS,
            )),
            Self::Organizations => Some((
                MICROSOFT_AUTH_URI_ORGANIZATIONS,
                MICROSOFT_TOKEN_ENDPOINT_ORGANIZATIONS,
            )),
            Self::Common => Some((MICROSOFT_AUTH_URI_COMMON, MICROSOFT_TOKEN_ENDPOINT_COMMON)),
            Self::SingleTenant => None,
        }
    }
}

/// Authorize + token endpoints for a single-tenant Microsoft app
/// registration ("Accounts in this organizational directory only"): the
/// tenant segment is the directory's tenant ID (GUID) or verified domain
/// (e.g. `contoso.com`) — `/organizations/` is invalid for this audience.
fn microsoft_tenant_endpoints(tenant: &str) -> (String, String) {
    (
        format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
        format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
    )
}

/// IMAP XOAUTH2 scope for Outlook / Office 365 plus `offline_access` for
/// refresh tokens (Microsoft's "authenticate an IMAP application by using
/// OAuth" docs; the connector keeps a refresh token with skew).
const MICROSOFT_IMAP_SCOPE: &str =
    "https://outlook.office.com/IMAP.AccessAsUser.All offline_access";
/// Email provider presets offered by the wizard (issue #400): each entry
/// pre-fills IMAP defaults + provider guidance; `Custom IMAP` keeps the
/// free-form flow. Presets are wizard-side defaults only — the backend stays
/// `imap` for every provider.
const EMAIL_PROVIDER_OPTIONS: [&str; 6] = [
    "Gmail",
    "Outlook / Office 365",
    "Yahoo",
    "Proton Mail (Bridge)",
    "iCloud",
    "Custom IMAP",
];
/// Calendar provider presets offered by the wizard (issue #400). Outlook /
/// Office 365 is deliberately absent: Microsoft exposes no public CalDAV
/// endpoint (the roadmap defers a Microsoft Graph calendar backend as a
/// follow-on), so a preset would always produce a broken connector.
const CALENDAR_PROVIDER_OPTIONS: [&str; 4] =
    ["Google Calendar", "iCloud", "Yahoo", "Custom CalDAV"];
/// Wizard sync-mode choices for the email IMAP profile (issue #397):
/// "Continuously — push" maps to `mode: auto` (IDLE when the server
/// advertises it), "Every N minutes — polling" maps to `mode: poll` plus a
/// `poll_interval_secs`.
const SYNC_MODE_PUSH: &str = "Continuously — push (recommended)";
const SYNC_MODE_POLL: &str = "Every N minutes — polling";
/// Polling presets plus the custom-option sentinel; `INTERVAL_MINUTES` holds
/// the matching minutes for the first four entries.
const INTERVAL_OPTIONS: [&str; 5] = [
    "5 minutes",
    "15 minutes",
    "30 minutes",
    "60 minutes",
    "Custom interval (minutes)",
];
const INTERVAL_MINUTES: [u64; 4] = [5, 15, 30, 60];
/// Wizard first-sync choice for the email IMAP profile (issue #397):
/// import the existing mailbox, or start from "now" (seed the cursor so the
/// first cycle skips existing mail).
const BACKFILL_IMPORT: &str = "Import existing mailbox content (recommended)";
const BACKFILL_NEW_ONLY: &str = "Only new content from now on";

/// Interactive prompt driver. Production uses `inquire`; tests inject a
/// scripted driver so the whole wizard (prompts → catalog → PKCE →
/// registration → credential ingest) is exercised without a TTY.
pub(crate) trait PromptDriver {
    /// Let the user pick one of `options`, returning its index.
    fn select(&self, message: &str, options: &[String]) -> Result<usize, String>;
    /// Free-text input with an optional default (empty answer → default).
    fn input(&self, message: &str, default: Option<&str>) -> Result<String, String>;
    /// Hidden input (secrets are never echoed to the terminal).
    fn password(&self, message: &str) -> Result<String, String>;
}

/// Production [`PromptDriver`] backed by `inquire`.
pub(crate) struct InquirePrompt;

impl PromptDriver for InquirePrompt {
    fn select(&self, message: &str, options: &[String]) -> Result<usize, String> {
        inquire::Select::new(message, options.to_vec())
            .prompt()
            .map(|choice| options.iter().position(|o| *o == choice).unwrap_or(0))
            .map_err(prompt_error)
    }

    fn input(&self, message: &str, default: Option<&str>) -> Result<String, String> {
        let mut prompt = inquire::Text::new(message);
        if let Some(default) = default {
            prompt = prompt.with_default(default);
        }
        prompt.prompt().map_err(prompt_error)
    }

    fn password(&self, message: &str) -> Result<String, String> {
        password_prompt(message).prompt().map_err(prompt_error)
    }
}

/// Build the production secret prompt, shared by the wizard's
/// `InquirePrompt` and the flag form's `prompt_secret` (DRY). Kept as a
/// separate function so tests can assert the prompt configuration without a
/// TTY. Confirmation is disabled (issue #399): inquire 0.9.4 enables it by
/// default, so the hidden secret prompts asked twice — the second masked
/// "Confirmation:" input looked like a hang right before the OAuth browser
/// opened. Secrets are typically pasted, the mismatch loop is more
/// confusing than a rare typo, and the connector auth step already fails
/// loudly with a clear error when a secret is wrong.
pub(crate) fn password_prompt(message: &str) -> inquire::Password<'_> {
    inquire::Password::new(message).without_confirmation()
}

/// Render an `inquire` error as a human message; aborts (Esc / Ctrl-C /
/// Ctrl-D) read as "canceled".
fn prompt_error(e: inquire::InquireError) -> String {
    match e {
        inquire::InquireError::OperationCanceled => "prompt canceled".to_string(),
        other => format!("prompt failed: {other}"),
    }
}

/// What credential the wizard acquired alongside the config, resolved
/// *before* anything is registered so an aborted flow creates nothing.
#[derive(Debug)]
pub(crate) enum WizardCredential {
    /// Run the interactive PKCE flow (browser + printed URL). The OAuth
    /// client secret (confidential clients only) is carried separately so
    /// it is never written into `config_json` — it goes to the daemon's
    /// secret store inside the credential bundle.
    OAuth { client_secret: Option<String> },
    /// App-password / API-token secret already prompted.
    Secret(String),
    /// No credential (e.g. local-filesystem backends).
    None,
}

/// `mimir connector add` with no arguments — interactive wizard.
///
/// Requires a TTY: with piped stdin there is no prompt channel, so the flag
/// form is the non-interactive path (a clear message points there instead of
/// failing mid-prompt).
pub async fn handle_connector_add_wizard(
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    if !std::io::stdin().is_terminal() {
        exit_with_error(
            "interactive mode requires a terminal — pass the connector type and options as arguments instead (see `mimir connector add --help`)",
        );
    }
    handle_connector_add_wizard_with_deps(json, transport, &InquirePrompt, &open_in_browser).await;
}

/// Testable core of [`handle_connector_add_wizard`]: `prompts` replaces the
/// terminal and `opener` drives the PKCE loopback (mirroring the `add`
/// flow's `_with_opener` split).
pub(crate) async fn handle_connector_add_wizard_with_deps(
    json: bool,
    transport: &crate::transport::DaemonTransport,
    prompts: &dyn PromptDriver,
    opener: &(dyn Fn(&str) + Send + Sync),
) {
    let client = make_client(transport);
    let catalog = client
        .connector_catalog()
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    if catalog.entries.is_empty() {
        exit_with_error(no_backends_message());
    }

    let entry = select_catalog_entry(&catalog, prompts);
    let display_name = prompts
        .input("Display name", Some(&title_case(&entry.connector_type)))
        .unwrap_or_else(|e| exit_with_error(e));
    let slug = prompts
        .input("Slug (unique identifier)", Some(&slugify(&display_name)))
        .unwrap_or_else(|e| exit_with_error(e));
    validate_slug(&slug);

    let (config, credential) =
        build_wizard_config(&entry, &slug, prompts).unwrap_or_else(|e| exit_with_error(e));
    // Capture the wizard's sync choices for the post-activation summary
    // (issue #397): the response carries the resolved mode, but the poll
    // interval and the backfill choice live only in the config we register.
    let sync_summary = wizard_sync_summary(&config);

    let (kind, oauth_bundle, secret) = match credential {
        WizardCredential::OAuth { client_secret } => {
            let bundle =
                run_oauth_flow_with_opener_and_secret(&config, client_secret.as_deref(), opener)
                    .await;
            (CredentialKind::OAuth, Some(bundle), None)
        }
        WizardCredential::Secret(secret) => {
            let kind = if config.pointer("/auth/kind").and_then(|v| v.as_str()) == Some("api_token")
            {
                CredentialKind::ApiToken
            } else {
                CredentialKind::AppPassword
            };
            (kind, None, Some(secret))
        }
        WizardCredential::None => (CredentialKind::None, None, None),
    };

    let output = register_and_ingest(
        &client,
        entry.connector_type.clone(),
        entry.backend.clone(),
        slug.clone(),
        display_name,
        config,
        kind,
        oauth_bundle,
        secret,
    )
    .await;

    // Issue #397: adding a connector should just work — once credential
    // ingest succeeds, activate it automatically (activation is the `resume`
    // action, A2 / #203). The runner's first cycle syncs immediately
    // (polling) or backfills the existing mailbox before blocking on IDLE
    // (push), so no manual `resume` / `sync` ceremony remains. The flag form
    // keeps the explicit lifecycle for scripts.
    let (resumed, activated) = match client.connector_resume(output.id).await {
        Ok(resumed) => (resumed, true),
        Err(error) => {
            eprintln!(
                "Warning: connector '{slug}' was registered but activation failed: {} — run `mimir connector resume {slug}` to activate it.",
                render_client_error(error)
            );
            (output, false)
        }
    };

    if json {
        print_json(&resumed);
        return;
    }
    println!(
        "Added connector '{}' ({} / {}, id {}, status {}, mode {}, auth {}).",
        resumed.slug,
        resumed.connector_type,
        resumed.backend,
        resumed.id,
        resumed.status,
        resumed.mode.as_deref().unwrap_or("-"),
        resumed.auth_state
    );
    if activated {
        if let Some(line) = sync_summary {
            println!("Syncing now: {line}.");
        }
    } else {
        println!("Activate it with `mimir connector resume {slug}` to start syncing.");
    }
    println!(
        "This connector imports data read-only by default — it does not write to the service on its own. Write-back actions run only when you explicitly invoke `mimir connector act {}`.",
        resumed.slug
    );
}

/// One-line description of what a wizard-configured connector will do once
/// activated, printed in the add summary (issue #397). `None` for profiles
/// that do not carry the wizard's sync-mode keys (calendar / photos) — the
/// summary's resolved `mode` column covers those.
pub(crate) fn wizard_sync_summary(config: &Value) -> Option<String> {
    match config.get("mode").and_then(Value::as_str) {
        Some("poll") => {
            let minutes = config
                .get("poll_interval_secs")
                .and_then(Value::as_u64)
                .map(|secs| secs / 60);
            Some(match minutes {
                Some(1) => "polling every 1 minute".to_string(),
                Some(n) => format!("polling every {n} minutes"),
                None => "polling on a fixed interval".to_string(),
            })
        }
        Some("auto") | Some("idle") => {
            let new_only = config.get("initial_backfill").and_then(Value::as_bool) == Some(false);
            Some(if new_only {
                "push — listening for new mail via IMAP IDLE (existing mailbox content skipped)"
                    .to_string()
            } else {
                "push — importing existing mailbox content, then listening for new mail via IMAP IDLE".to_string()
            })
        }
        _ => None,
    }
}

/// Let the user pick a `(type, backend)` pair from the daemon's catalog,
/// rendering each as `Type (backend)`.
fn select_catalog_entry(
    catalog: &mimir_api_types::ConnectorCatalogResponse,
    prompts: &dyn PromptDriver,
) -> mimir_api_types::ConnectorCatalogEntry {
    let options = catalog
        .entries
        .iter()
        .map(|e| format!("{} ({})", title_case(&e.connector_type), e.backend))
        .collect::<Vec<_>>();
    let index = prompts
        .select("Connector type", &options)
        .unwrap_or_else(|e| exit_with_error(e));
    catalog.entries[index].clone()
}

/// Build the per-backend `config_json` for the wizard-selected pair, plus
/// which credential the flow must gather. Returns a `String` error (rendered
/// by the caller as an exit) when the pair has no wizard profile yet — the
/// flag form remains the escape hatch for backends the wizard does not know.
pub(crate) fn build_wizard_config(
    entry: &mimir_api_types::ConnectorCatalogEntry,
    slug: &str,
    prompts: &dyn PromptDriver,
) -> Result<(Value, WizardCredential), String> {
    match (entry.connector_type.as_str(), entry.backend.as_str()) {
        ("email", "imap") => email_imap_config(prompts),
        ("calendar", "caldav") => calendar_provider_config(prompts),
        ("photos", "local") => photos_config(slug, prompts),
        (connector_type, backend) => Err(format!(
            "no interactive profile for '{connector_type}/{backend}' yet — use the flag form instead, e.g. `mimir connector add {connector_type} --backend {backend} --config-json '{{...}}'`"
        )),
    }
}

/// Email IMAP wizard profile (issue #400): a provider list (Gmail, Outlook /
/// Office 365, Yahoo, Proton Mail via the Bridge, iCloud, or a custom IMAP
/// server) pre-fills the IMAP defaults and provider-specific guidance; the
/// sync-mode / backfill questions (issue #397) apply to every provider. The
/// backend stays `imap` for all of them — presets are wizard-side defaults,
/// never new backends.
fn email_imap_config(prompts: &dyn PromptDriver) -> Result<(Value, WizardCredential), String> {
    let options = EMAIL_PROVIDER_OPTIONS.map(str::to_string).to_vec();
    let provider = prompts.select("Email provider", &options)?;
    let mut preset = email_preset(provider);

    // Issue #467: the Outlook / Office 365 preset asks which Microsoft
    // account type the app registration targets — the identity-platform
    // endpoints differ per audience (`/consumers/`, `/organizations/`,
    // `/common/`, or the tenant ID/domain for single-tenant apps) and a
    // mismatch fails the authorize request, so the matching endpoints are
    // pre-filled before the endpoint prompts run.
    if let Some(oauth) = preset
        .oauth
        .as_mut()
        .filter(|oauth| oauth.microsoft_account_type_prompt)
    {
        let options = MICROSOFT_ACCOUNT_TYPE_OPTIONS
            .map(|account_type| account_type.label().to_string())
            .to_vec();
        let account_type =
            MicrosoftAccountType::from_index(prompts.select("Microsoft account type", &options)?);
        match account_type.fixed_endpoints() {
            Some((auth_uri, token_endpoint)) => {
                oauth.auth_uri = Some(auth_uri.to_string());
                oauth.token_endpoint = Some(token_endpoint.to_string());
            }
            None => {
                // Single-tenant app registration: both endpoints embed the
                // tenant ID or domain, so collect it before the endpoint
                // prompts run.
                let tenant = required(
                    prompts.input(
                        "Tenant ID or domain (the app registration's Entra directory, e.g. contoso.com or a tenant GUID)",
                        None,
                    ),
                    "tenant ID or domain",
                )?;
                let (auth_uri, token_endpoint) = microsoft_tenant_endpoints(&tenant);
                oauth.auth_uri = Some(auth_uri);
                oauth.token_endpoint = Some(token_endpoint);
            }
        }
    }

    let host_message = if preset.host_help.is_empty() {
        "IMAP server host".to_string()
    } else {
        format!("IMAP server host ({})", preset.host_help)
    };
    let host = required(
        prompts.input(&host_message, preset.host_default),
        "IMAP server host",
    )?;
    let port = parse_port(prompts.input(
        "IMAP port (blank = 993)",
        Some(&preset.port_default.to_string()),
    )?)?;
    let mailbox = required(prompts.input("Mailbox", Some("INBOX")), "Mailbox")?;
    let username = required(
        prompts.input("Account email (IMAP login)", None),
        "Account email",
    )?;
    let sync = email_sync_questions(prompts)?;

    // Authentication per provider: Gmail offers OAuth first (Google retired
    // plain password IMAP; app passwords are the fallback), Outlook is
    // OAuth-only (Microsoft retired app passwords for IMAP), and Yahoo /
    // Proton / iCloud are app-password only (issue #400).
    let app_password_hint = preset.app_password_hint.unwrap_or("App password");
    let (auth, credential) = if let Some(oauth) = preset.oauth {
        if preset.app_password_hint.is_none() {
            // OAuth-only preset (Outlook / Office 365): no app-password path
            // is offered because Microsoft retired basic-auth app passwords
            // for Outlook.com and Exchange Online IMAP.
            email_oauth_questions(prompts, &oauth, &username)?
        } else {
            let oauth_first = !preset.app_password_first;
            let chosen_oauth = prompts.select(
                "Authentication",
                &[
                    if oauth_first {
                        "OAuth 2.0 — browser login (recommended)".to_string()
                    } else {
                        "App password (recommended — no app registration needed)".to_string()
                    },
                    if oauth_first {
                        "App password".to_string()
                    } else {
                        "OAuth 2.0 — browser login".to_string()
                    },
                ],
            )? == if oauth_first { 0 } else { 1 };
            if chosen_oauth {
                email_oauth_questions(prompts, &oauth, &username)?
            } else {
                let password =
                    required_secret(prompts.password(app_password_hint), "App password")?;
                (
                    json!({"kind": "app_password", "username": username}),
                    WizardCredential::Secret(password),
                )
            }
        }
    } else {
        let password = required_secret(prompts.password(app_password_hint), "App password")?;
        (
            json!({"kind": "app_password", "username": username}),
            WizardCredential::Secret(password),
        )
    };
    Ok((email_config(&host, port, &mailbox, &sync, auth), credential))
}

/// Resolve the OAuth branch for an email provider: prompts the endpoints and
/// scopes (pre-filled for known providers, free-form for custom IMAP) and
/// returns the auth block plus the credential to carry out of the flow. The
/// OAuth client secret never enters `config_json` — it travels in the
/// credential bundle.
fn email_oauth_questions(
    prompts: &dyn PromptDriver,
    preset: &EmailOAuthPreset,
    username: &str,
) -> Result<(Value, WizardCredential), String> {
    let auth_uri = required(
        prompts.input("Authorization endpoint URL", preset.auth_uri.as_deref()),
        "Authorization endpoint URL",
    )?;
    let token_endpoint = required(
        prompts.input("Token endpoint URL", preset.token_endpoint.as_deref()),
        "Token endpoint URL",
    )?;
    let client_id = required(
        prompts.input(preset.client_id_help, None),
        "OAuth client ID",
    )?;
    let client_secret = prompts.password("OAuth client secret (blank if none)")?;
    let client_secret = if client_secret.trim().is_empty() {
        None
    } else {
        Some(client_secret)
    };
    let scopes_raw = required(
        prompts.input(
            "OAuth scopes (comma or space-separated)",
            preset.default_scopes,
        ),
        "OAuth scopes",
    )?;
    let scopes = parse_scopes_required(&scopes_raw)?;
    let auth = json!({
        "kind": "oauth",
        "username": username,
        "auth_uri": auth_uri,
        "token_endpoint": token_endpoint,
        "client_id": client_id,
        "scopes": scopes,
    });
    Ok((auth, WizardCredential::OAuth { client_secret }))
}

/// Assemble the shared email `config_json` from the prompted values (issue
/// #397): `mode` is `auto` (IDLE when advertised) for push, `poll` plus
/// `poll_interval_secs` for polling, and `initial_backfill` carries the
/// existing-content choice.
fn email_config(
    host: &str,
    port: Option<u16>,
    mailbox: &str,
    sync: &EmailSyncChoices,
    auth: Value,
) -> Value {
    let mut config = json!({
        "host": host,
        "mailbox": mailbox,
        "mode": if sync.push { "auto" } else { "poll" },
        "initial_backfill": sync.backfill,
        "auth": auth,
    });
    if let Some(port) = port {
        config["port"] = json!(port);
    }
    if let Some(secs) = sync.poll_interval_secs {
        config["poll_interval_secs"] = json!(secs);
    }
    config
}

/// The wizard's sync decisions for an email connector (issue #397), shared
/// by every provider preset: push (continuous, `mode: auto` → IDLE when the
/// server advertises it) vs polling with a preset or custom interval, plus
/// whether the first sync imports the existing mailbox or starts from "now".
struct EmailSyncChoices {
    push: bool,
    poll_interval_secs: Option<u64>,
    backfill: bool,
}

fn email_sync_questions(prompts: &dyn PromptDriver) -> Result<EmailSyncChoices, String> {
    let push = prompts.select(
        "Sync mode",
        &[SYNC_MODE_PUSH.to_string(), SYNC_MODE_POLL.to_string()],
    )? == 0;
    let poll_interval_secs = if push {
        None
    } else {
        let options = INTERVAL_OPTIONS.map(str::to_string).to_vec();
        let choice = prompts.select("Poll interval", &options)?;
        let minutes = if choice < INTERVAL_MINUTES.len() {
            INTERVAL_MINUTES[choice]
        } else {
            parse_interval_minutes(prompts.input("Poll interval (minutes)", None)?)?
        };
        Some(interval_secs(minutes)?)
    };
    let backfill = prompts.select(
        "Existing mailbox content",
        &[BACKFILL_IMPORT.to_string(), BACKFILL_NEW_ONLY.to_string()],
    )? == 0;
    Ok(EmailSyncChoices {
        push,
        poll_interval_secs,
        backfill,
    })
}

/// Wizard-side email provider defaults (issue #400). Every preset is a
/// default + guidance bundle: the backend stays `imap` for all of them.
struct EmailPreset {
    /// Default IMAP host; `None` for the free-form custom flow.
    host_default: Option<&'static str>,
    /// Extra guidance for the host prompt (e.g. Proton Mail Bridge).
    host_help: &'static str,
    port_default: u16,
    /// Where to create the app password, embedded in the prompt label;
    /// `None` for an OAuth-only preset (no app-password path is offered).
    app_password_hint: Option<&'static str>,
    /// OAuth preset; `None` = the provider is app-password only.
    oauth: Option<EmailOAuthPreset>,
    /// `true` lists the app password first (the zero-setup path); `false`
    /// lists OAuth first (Gmail — Google retired plain password IMAP;
    /// Outlook is OAuth-only, so it never reaches the list).
    app_password_first: bool,
}

/// OAuth endpoint/scope defaults for one email provider (issue #400).
struct EmailOAuthPreset {
    /// Default authorization endpoint; `None` for the free-form custom
    /// flow. Owned so the Microsoft preset can pre-fill tenant-specific
    /// (single-tenant) endpoints built from the collected tenant.
    auth_uri: Option<String>,
    /// Default token endpoint; `None` for the free-form custom flow.
    token_endpoint: Option<String>,
    /// Default scope string; `None` for the free-form custom flow.
    default_scopes: Option<&'static str>,
    /// Where the user obtains the OAuth client ID.
    client_id_help: &'static str,
    /// `true` for Microsoft presets (issue #467): the wizard asks which
    /// account audience the app registration targets and re-targets the
    /// endpoint defaults to `/consumers/`, `/organizations/`, or
    /// `/common/` (or builds tenant-specific ones for single-tenant apps)
    /// accordingly.
    microsoft_account_type_prompt: bool,
}

/// Provider → defaults table for the email wizard (issue #400). The index
/// matches [`EMAIL_PROVIDER_OPTIONS`]; the trailing arm is `Custom IMAP`.
fn email_preset(provider: usize) -> EmailPreset {
    match provider {
        // Gmail: Google endpoints pre-filled; OAuth first (recommended),
        // app password as the fallback.
        0 => EmailPreset {
            host_default: Some("imap.gmail.com"),
            host_help: "",
            port_default: 993,
            app_password_hint: Some(
                "App password (Google Account → Security → 2-Step Verification → App passwords)",
            ),
            oauth: Some(EmailOAuthPreset {
                auth_uri: Some(GOOGLE_AUTH_URI.to_string()),
                token_endpoint: Some(GOOGLE_TOKEN_ENDPOINT.to_string()),
                default_scopes: Some(GMAIL_SCOPE),
                client_id_help: "OAuth client ID (Google Cloud Console → Credentials → OAuth client)",
                microsoft_account_type_prompt: false,
            }),
            app_password_first: false,
        },
        // Outlook / Office 365: Microsoft identity platform endpoints
        // pre-filled; OAuth 2.0 only — Microsoft retired basic-auth app
        // passwords for Outlook.com and Exchange Online IMAP, so no
        // app-password path is offered. The account-type question (issue
        // #467) re-targets the endpoints to the audience the app
        // registration allows; single-tenant registrations get
        // tenant-specific endpoints built from the collected tenant.
        1 => EmailPreset {
            host_default: Some("outlook.office365.com"),
            host_help: "",
            port_default: 993,
            app_password_hint: None,
            oauth: Some(EmailOAuthPreset {
                auth_uri: Some(MICROSOFT_AUTH_URI_COMMON.to_string()),
                token_endpoint: Some(MICROSOFT_TOKEN_ENDPOINT_COMMON.to_string()),
                default_scopes: Some(MICROSOFT_IMAP_SCOPE),
                client_id_help: "OAuth client ID (Entra ID app registration; 'Supported account types' must match the Microsoft account type chosen in this wizard — single-tenant 'this organizational directory only' apps need the tenant ID or domain too; register the loopback redirect URI http://localhost/callback)",
                microsoft_account_type_prompt: true,
            }),
            app_password_first: false,
        },
        // Yahoo: app password only (2-Step Verification required).
        2 => EmailPreset {
            host_default: Some("imap.mail.yahoo.com"),
            host_help: "",
            port_default: 993,
            app_password_hint: Some(
                "App password (Yahoo Account Security → Generate app password; requires 2-Step Verification)",
            ),
            oauth: None,
            app_password_first: true,
        },
        // Proton Mail: no public IMAP — the Bridge exposes a local IMAP
        // server on 127.0.0.1:1143 and issues its own app passwords.
        3 => EmailPreset {
            host_default: Some("127.0.0.1"),
            host_help: "Proton Mail Bridge exposes a local IMAP server — start the Bridge and log in first",
            port_default: 1143,
            app_password_hint: Some(
                "App password (Proton Mail Bridge → Generate Proton Mail Bridge app password)",
            ),
            oauth: None,
            app_password_first: true,
        },
        // iCloud: app password only (two-factor authentication required).
        4 => EmailPreset {
            host_default: Some("imap.mail.me.com"),
            host_help: "",
            port_default: 993,
            app_password_hint: Some(
                "App password (appleid.apple.com → Sign-In & Security → App-Specific Passwords; requires two-factor authentication)",
            ),
            oauth: None,
            app_password_first: true,
        },
        // Custom IMAP: free-form host/port/mailbox; app password first, or
        // OAuth with user-supplied endpoints.
        _ => EmailPreset {
            host_default: None,
            host_help: "",
            port_default: 993,
            app_password_hint: Some("App password"),
            oauth: Some(EmailOAuthPreset {
                auth_uri: None,
                token_endpoint: None,
                default_scopes: None,
                client_id_help: "OAuth client ID",
                microsoft_account_type_prompt: false,
            }),
            app_password_first: true,
        },
    }
}

/// Parse a custom poll interval in whole minutes; rejects empty,
/// non-numeric, and zero values before anything is registered.
fn parse_interval_minutes(raw: String) -> Result<u64, String> {
    let minutes = raw
        .trim()
        .parse::<u64>()
        .map_err(|_| "invalid poll interval (whole minutes, at least 1)".to_string())?;
    if minutes == 0 {
        return Err("poll interval must be at least 1 minute".to_string());
    }
    Ok(minutes)
}

/// Convert a validated whole-minute poll interval to seconds; rejects a
/// value whose `* 60` would overflow `u64` (a user-typed custom interval)
/// before anything is registered.
fn interval_secs(minutes: u64) -> Result<u64, String> {
    minutes
        .checked_mul(60)
        .ok_or_else(|| "poll interval is too large".to_string())
}

/// Calendar (CalDAV) wizard profile (issue #400): a provider list (Google
/// Calendar, iCloud, Yahoo, or a custom CalDAV server) pre-fills the
/// collection-URL defaults and provider guidance. Outlook / Office 365 is
/// absent because Microsoft exposes no public CalDAV endpoint (the roadmap
/// defers a Microsoft Graph calendar backend as a follow-on).
fn calendar_provider_config(
    prompts: &dyn PromptDriver,
) -> Result<(Value, WizardCredential), String> {
    let options = CALENDAR_PROVIDER_OPTIONS.map(str::to_string).to_vec();
    let provider = prompts.select("Calendar provider", &options)?;
    match provider {
        // Google Calendar: no app passwords — OAuth with the Google
        // endpoints pre-filled. The primary calendar's CalDAV collection URL
        // embeds the account email, so the default is computed after the
        // username prompt (developers.google.com/calendar/caldav).
        0 => {
            let username = required(prompts.input("Google account email", None), "Username")?;
            let default_url =
                format!("https://apidata.googleusercontent.com/caldav/v2/{username}/events");
            let calendar_url = required(
                prompts.input(
                    "Calendar (CalDAV) collection URL (defaults to the primary calendar)",
                    Some(&default_url),
                ),
                "Calendar URL",
            )?;
            let (auth, credential) = calendar_oauth_questions(
                prompts,
                &username,
                GOOGLE_AUTH_URI,
                GOOGLE_TOKEN_ENDPOINT,
                "OAuth client ID (Google Cloud Console → Credentials → OAuth client)",
            )?;
            Ok((
                json!({ "calendar_url": calendar_url, "auth": auth }),
                credential,
            ))
        }
        // iCloud: app-specific password only (two-factor authentication
        // required); the per-account collection URL is not predictable, so
        // the server URL is the default and the prompt says where to find
        // the full one.
        1 => caldav_app_password_provider(
            prompts,
            "Calendar (CalDAV) collection URL (iCloud: paste your calendar's full URL, e.g. https://caldav.icloud.com/<id>/calendars/<name>/)",
            "https://caldav.icloud.com/",
            "Apple ID email",
            "App password (appleid.apple.com → Sign-In & Security → App-Specific Passwords; requires two-factor authentication)",
        ),
        // Yahoo: app password only (2-Step Verification required).
        2 => caldav_app_password_provider(
            prompts,
            "Calendar (CalDAV) collection URL (Yahoo: your account's calendar URL under caldav.calendar.yahoo.com)",
            "https://caldav.calendar.yahoo.com/",
            "Yahoo email",
            "App password (Yahoo Account Security → Generate app password; requires 2-Step Verification)",
        ),
        // Custom CalDAV: the original free-form flow, unchanged.
        _ => {
            let calendar_url = required(
                prompts.input("Calendar (CalDAV) collection URL", None),
                "Calendar URL",
            )?;
            let username = required(prompts.input("Username", None), "Username")?;
            let oauth = prompts.select(
                "Authentication",
                &[
                    "App password".to_string(),
                    "OAuth 2.0 — browser login".to_string(),
                ],
            )? == 1;
            if oauth {
                let auth_uri = required(
                    prompts.input("Authorization endpoint URL", None),
                    "Authorization endpoint URL",
                )?;
                let token_endpoint = required(
                    prompts.input("Token endpoint URL", None),
                    "Token endpoint URL",
                )?;
                let (auth, credential) = calendar_oauth_questions(
                    prompts,
                    &username,
                    &auth_uri,
                    &token_endpoint,
                    "OAuth client ID",
                )?;
                Ok((
                    json!({ "calendar_url": calendar_url, "auth": auth }),
                    credential,
                ))
            } else {
                let password = required_secret(prompts.password("App password"), "App password")?;
                Ok((
                    json!({
                        "calendar_url": calendar_url,
                        "auth": {"kind": "app_password", "username": username},
                    }),
                    WizardCredential::Secret(password),
                ))
            }
        }
    }
}

/// Shared app-password CalDAV arm (iCloud, Yahoo): prompt the collection
/// URL, the account, and the app password, then assemble the config. The
/// two presets differ only in the URL message/default, the username label,
/// and the password hint.
fn caldav_app_password_provider(
    prompts: &dyn PromptDriver,
    url_message: &str,
    url_default: &str,
    username_message: &str,
    password_hint: &str,
) -> Result<(Value, WizardCredential), String> {
    let calendar_url = required(
        prompts.input(url_message, Some(url_default)),
        "Calendar URL",
    )?;
    let username = required(prompts.input(username_message, None), "Username")?;
    let password = required_secret(prompts.password(password_hint), "App password")?;
    Ok((
        json!({
            "calendar_url": calendar_url,
            "auth": {"kind": "app_password", "username": username},
        }),
        WizardCredential::Secret(password),
    ))
}

/// Prompt the OAuth client credentials for a CalDAV flow and assemble the
/// auth block; the client secret never enters `config_json` — it travels in
/// the credential bundle. Shared by the Google Calendar preset (endpoints
/// pre-filled) and the custom CalDAV flow (user-supplied endpoints). A blank
/// scopes answer keeps the [`CALDAV_SCOPE`] default, but a non-blank answer
/// that parses to zero scopes (e.g. `", ,"`) is rejected so a scope-less
/// authorize request is never built (mirrors `email_oauth_questions`).
fn calendar_oauth_questions(
    prompts: &dyn PromptDriver,
    username: &str,
    auth_uri: &str,
    token_endpoint: &str,
    client_id_help: &str,
) -> Result<(Value, WizardCredential), String> {
    let client_id = required(prompts.input(client_id_help, None), "OAuth client ID")?;
    let client_secret = prompts.password("OAuth client secret (blank if none)")?;
    let client_secret = if client_secret.trim().is_empty() {
        None
    } else {
        Some(client_secret)
    };
    let scopes_raw = prompts.input(
        "OAuth scopes (comma or space-separated)",
        Some(CALDAV_SCOPE),
    )?;
    let scopes = parse_scopes_required(&scopes_raw)?;
    let auth = json!({
        "kind": "oauth",
        "username": username,
        "auth_uri": auth_uri,
        "token_endpoint": token_endpoint,
        "client_id": client_id,
        "scopes": scopes,
    });
    Ok((auth, WizardCredential::OAuth { client_secret }))
}

/// Split a user-entered scope list on commas and/or whitespace into a JSON
/// string array, dropping empty segments.
pub(crate) fn parse_scopes(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a user-entered OAuth scope list and reject a list that parses to
/// zero scopes, so a scope-less authorize request is never built. Shared by
/// the email and calendar OAuth prompts; the email prompt additionally
/// rejects a blank answer via [`required`] (the custom IMAP preset has no
/// default scope), while a blank calendar answer keeps the [`CALDAV_SCOPE`]
/// default before this runs.
fn parse_scopes_required(raw: &str) -> Result<Vec<String>, String> {
    let scopes = parse_scopes(raw);
    if scopes.is_empty() {
        return Err("OAuth scopes is required".to_string());
    }
    Ok(scopes)
}

/// Local-photos wizard: a directory to watch; no credential.
fn photos_config(
    slug: &str,
    prompts: &dyn PromptDriver,
) -> Result<(Value, WizardCredential), String> {
    let watch_dir = required(
        prompts.input("Directory to watch (absolute path)", None),
        "Directory to watch",
    )?;
    let owner_name = prompts.input("Photo owner name (blank = slug)", Some(slug))?;
    let mut config = json!({ "watch_dir": watch_dir });
    if !owner_name.is_empty() {
        config["owner_name"] = json!(owner_name);
    }
    Ok((config, WizardCredential::None))
}

/// Prompt for a required value, mapping a canceled/empty answer to a clear
/// error before anything is registered.
fn required(prompt: Result<String, String>, label: &str) -> Result<String, String> {
    let value = prompt?;
    if value.trim().is_empty() {
        return Err(format!("{label} is required"));
    }
    Ok(value.trim().to_string())
}

/// Prompt for a required secret, rejecting a canceled/empty/whitespace-only
/// answer before anything is registered. Unlike [`required`], the value is
/// preserved unchanged (no trimming) so a legitimate password is never
/// altered.
fn required_secret(prompt: Result<String, String>, label: &str) -> Result<String, String> {
    let value = prompt?;
    if value.trim().is_empty() {
        return Err(format!("{label} is required"));
    }
    Ok(value)
}

/// Parse the optional IMAP port (blank → `None`); a non-numeric or
/// out-of-range value fails before registration.
fn parse_port(raw: String) -> Result<Option<u16>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let port = trimmed
        .parse::<u16>()
        .map_err(|_| format!("invalid port '{trimmed}' (use 1–65535)"))?;
    if port == 0 {
        return Err(format!("invalid port '{trimmed}' (use 1–65535)"));
    }
    Ok(Some(port))
}

/// Slugify a display name into a connector slug: lowercase ASCII
/// alphanumerics, runs of other characters collapse to a single hyphen.
/// Used to default the slug prompt from the user's name.
pub(crate) fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Validate the wizard's final slug client-side (mirroring the daemon's
/// secret-store slug rule): non-empty ASCII alphanumeric, `_` and `-`.
fn validate_slug(slug: &str) {
    if slug.is_empty() {
        exit_with_error("slug must not be empty");
    }
    if slug.len() > 128 {
        exit_with_error(format!("slug '{slug}' is longer than 128 characters"));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        exit_with_error(format!(
            "slug '{slug}' may contain only ASCII letters, digits, '_' and '-'"
        ));
    }
}
