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
/// Wizard sync-mode choices for the Gmail IMAP profile (issue #397):
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
/// Wizard first-sync choice for the Gmail IMAP profile (issue #397):
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
pub async fn handle_connector_add_wizard(json: bool, base_url: &str) {
    if !std::io::stdin().is_terminal() {
        exit_with_error(
            "interactive mode requires a terminal — pass the connector type and options as arguments instead (see `mimir connector add --help`)",
        );
    }
    handle_connector_add_wizard_with_deps(json, base_url, &InquirePrompt, &open_in_browser).await;
}

/// Testable core of [`handle_connector_add_wizard`]: `prompts` replaces the
/// terminal and `opener` drives the PKCE loopback (mirroring the `add`
/// flow's `_with_opener` split).
pub(crate) async fn handle_connector_add_wizard_with_deps(
    json: bool,
    base_url: &str,
    prompts: &dyn PromptDriver,
    opener: &(dyn Fn(&str) + Send + Sync),
) {
    let client = make_client(base_url);
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
        ("gmail", "imap") => gmail_imap_config(prompts),
        ("calendar", "caldav") => caldav_config(prompts),
        ("photos", "local") => photos_config(slug, prompts),
        (connector_type, backend) => Err(format!(
            "no interactive profile for '{connector_type}/{backend}' yet — use the flag form instead, e.g. `mimir connector add {connector_type} --backend {backend} --config-json '{{...}}'`"
        )),
    }
}

/// Gmail IMAP wizard profile: Google defaults for host/port/mailbox, OAuth
/// (browser login, recommended — Google has retired password IMAP) with the
/// Google endpoints pre-filled, or app password as a fallback.
fn gmail_imap_config(prompts: &dyn PromptDriver) -> Result<(Value, WizardCredential), String> {
    let host = required(
        prompts.input("IMAP server host", Some("imap.gmail.com")),
        "IMAP server host",
    )?;
    let port = parse_port(prompts.input("IMAP port (blank = 993)", Some("993"))?)?;
    let mailbox = required(prompts.input("Mailbox", Some("INBOX")), "Mailbox")?;
    let username = required(
        prompts.input("Account email (IMAP login)", None),
        "Account email",
    )?;
    // Issue #397: ask the sync-mode decision up front — push (continuous,
    // `mode: auto` → IDLE when advertised) vs polling with a preset or
    // custom interval — plus whether the first sync imports the existing
    // mailbox or starts from "now".
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
    let oauth = prompts.select(
        "Authentication",
        &[
            "OAuth 2.0 — browser login (recommended)".to_string(),
            "App password".to_string(),
        ],
    )? == 0;
    if oauth {
        let client_id = required(
            prompts.input(
                "OAuth client ID (Google Cloud Console → Credentials → OAuth client)",
                None,
            ),
            "OAuth client ID",
        )?;
        let client_secret = prompts.password("OAuth client secret (blank if none)")?;
        let client_secret = if client_secret.trim().is_empty() {
            None
        } else {
            Some(client_secret)
        };
        let auth = json!({
            "kind": "oauth",
            "username": username,
            "auth_uri": GOOGLE_AUTH_URI,
            "token_endpoint": GOOGLE_TOKEN_ENDPOINT,
            "client_id": client_id,
            "scopes": [GMAIL_SCOPE],
        });
        let mut config = json!({
            "host": host,
            "mailbox": mailbox,
            "mode": if push { "auto" } else { "poll" },
            "initial_backfill": backfill,
            "auth": auth,
        });
        if let Some(port) = port {
            config["port"] = json!(port);
        }
        if let Some(secs) = poll_interval_secs {
            config["poll_interval_secs"] = json!(secs);
        }
        Ok((config, WizardCredential::OAuth { client_secret }))
    } else {
        let password = required_secret(
            prompts.password(
                "App password (Google Account → Security → 2-Step Verification → App passwords)",
            ),
            "App password",
        )?;
        let mut config = json!({
            "host": host,
            "mailbox": mailbox,
            "mode": if push { "auto" } else { "poll" },
            "initial_backfill": backfill,
            "auth": {"kind": "app_password", "username": username},
        });
        if let Some(port) = port {
            config["port"] = json!(port);
        }
        if let Some(secs) = poll_interval_secs {
            config["poll_interval_secs"] = json!(secs);
        }
        Ok((config, WizardCredential::Secret(password)))
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

/// CalDAV calendar wizard: collection URL + username, then app password
/// (the common provider path) or OAuth.
fn caldav_config(prompts: &dyn PromptDriver) -> Result<(Value, WizardCredential), String> {
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
        let client_id = required(prompts.input("OAuth client ID", None), "OAuth client ID")?;
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
        let auth = json!({
            "kind": "oauth",
            "username": username,
            "auth_uri": auth_uri,
            "token_endpoint": token_endpoint,
            "client_id": client_id,
            "scopes": parse_scopes(&scopes_raw),
        });
        Ok((
            json!({ "calendar_url": calendar_url, "auth": auth }),
            WizardCredential::OAuth { client_secret },
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

/// Split a user-entered scope list on commas and/or whitespace into a JSON
/// string array, dropping empty segments. An empty answer yields no scopes
/// (the provider then rejects the authorize request, but the caller's
/// default prompt makes that a deliberate choice).
pub(crate) fn parse_scopes(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
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
