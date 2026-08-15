//! CLI handlers for the `connector` command group (Phase 3 A3 / issue #204).
//!
//! Every subcommand talks to the daemon over HTTP via `mimir-client` — the
//! same pattern as `mimir kb`. One module per concern: [`add`] for
//! registration + credential ingest, [`auth`] for re-runnable credential
//! ingest on an existing instance, [`oauth`] for the shared interactive
//! PKCE flow (A4 / #205), [`query`] for list/status, [`lifecycle`] for
//! pause/resume/remove/forget, [`sync`] for manual sync triggers, and
//! [`actions`] for write-back dispatch. Shared helpers (slug resolution,
//! duration/config parsing, prompting, rendering) live here.

use colored::Colorize;
use is_terminal::IsTerminal;
use mimir_api_types::ConnectorResponse;
use mimir_client::MimirClient;

pub(crate) use crate::cli_util::{exit_with_error, make_client, print_json};

mod actions;
mod add;
mod auth;
mod catalog;
mod lifecycle;
mod oauth;
mod query;
mod sync;
#[cfg(test)]
mod tests;

pub use actions::handle_connector_act;
pub use add::handle_connector_add;
pub use auth::handle_connector_auth;
pub use catalog::handle_connector_catalog;
pub use lifecycle::{
    handle_connector_forget, handle_connector_pause, handle_connector_remove,
    handle_connector_resume,
};
pub use query::{handle_connector_list, handle_connector_status};
pub use sync::handle_connector_sync;

/// Resolve a connector slug to its full row, exiting with a clear message
/// when no registered instance matches.
///
/// The daemon has no by-slug route, so this lists and filters client-side —
/// bounded by the instance count and a single round trip (A3 / #204).
async fn resolve_connector(client: &MimirClient, slug: &str) -> ConnectorResponse {
    let list = client
        .connectors()
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    list.connectors
        .into_iter()
        .find(|c| c.slug == slug)
        .unwrap_or_else(|| exit_with_error(format!("connector '{slug}' not found")))
}

/// Render a client error for the user, unwrapping the daemon's structured
/// `ApiError` JSON body when present so the message is the human detail
/// rather than raw JSON.
fn render_client_error(e: mimir_client::ClientError) -> String {
    match &e {
        mimir_client::ClientError::Server { status, message } => {
            format!("server error {status}: {}", server_error_detail(message))
        }
        _ => e.to_string(),
    }
}

/// Extract the `error` field of the daemon's `ApiError` JSON body, falling
/// back to the raw message when the body is not structured JSON.
fn server_error_detail(message: &str) -> String {
    serde_json::from_str::<serde_json::Value>(message)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| message.to_string())
}

/// Whether a 409 sync response is the "connector not running" error, so the
/// CLI can add the activation hint (`mimir connector resume <slug>`).
fn is_connector_not_running(message: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(message)
        .ok()
        .is_some_and(|v| v.get("code").and_then(|c| c.as_str()) == Some("CONNECTOR_NOT_RUNNING"))
}

/// Parse a human duration (`30s`, `5m`, `12h`, `7d`) or a bare integer of
/// seconds into seconds.
fn parse_duration(input: &str) -> Result<u64, String> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(secs);
    }
    // Peel the trailing unit letter via chars so a non-ASCII suffix cannot
    // land on a non-char-boundary `split_at` panic.
    let mut chars = s.chars();
    let unit = chars.next_back();
    let number = chars.as_str();
    let value: u64 = number
        .parse()
        .map_err(|_| format!("invalid duration '{input}' (use e.g. 30s, 5m, 12h, 7d)"))?;
    let multiplier = match unit {
        Some('s') => 1u64,
        Some('m') => 60,
        Some('h') => 3_600,
        Some('d') => 86_400,
        _ => {
            return Err(format!(
                "invalid duration unit in '{input}' (use s, m, h, d)"
            ));
        }
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("duration '{input}' overflows seconds"))
}

/// Merge `--config-json` (base) with positional `key=value` pairs (overrides)
/// into one configuration object.
///
/// Dotted keys nest (`auth.kind=app_password` → `{"auth": {"kind":
/// "app_password"}}`); scalar values are parsed as booleans, numbers, or
/// strings. The daemon remains the source of truth for schema validation at
/// instance construction (A3 / #204).
fn merge_config(pairs: &[String], config_json: Option<&str>) -> Result<serde_json::Value, String> {
    let mut config = match config_json {
        Some(raw) => {
            serde_json::from_str(raw).map_err(|e| format!("invalid --config-json: {e}"))?
        }
        None => serde_json::json!({}),
    };
    if !config.is_object() {
        return Err("--config-json must be a JSON object".to_string());
    }
    for pair in pairs {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| format!("config must be `key=value`, got '{pair}'"))?;
        if key.is_empty() {
            return Err(format!("config key cannot be empty (got '{pair}')"));
        }
        if key.split('.').any(|part| part.is_empty()) {
            return Err(format!(
                "config key cannot contain empty path segments (got '{pair}')"
            ));
        }
        set_dotted_path(&mut config, key, parse_config_scalar(raw));
    }
    Ok(config)
}

/// Parse a `key=value` config value as a boolean, number, JSON array/object,
/// or string. A value wrapped in double quotes is always a string
/// (`account="0755"` keeps the leading zero instead of becoming the number
/// 755); a value that starts with `[` or `{` is parsed as JSON (e.g.
/// `auth.scopes=["a","b"]`), falling back to a plain string when the JSON
/// does not parse (issue #289).
fn parse_config_scalar(raw: &str) -> serde_json::Value {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return serde_json::Value::String(raw[1..raw.len() - 1].to_string());
    }
    match raw {
        "true" => return serde_json::Value::Bool(true),
        "false" => return serde_json::Value::Bool(false),
        _ => {}
    }
    if let Ok(number) = raw.parse::<i64>() {
        return serde_json::Value::Number(number.into());
    }
    if let Ok(number) = raw.parse::<f64>()
        && number.is_finite()
    {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return serde_json::Value::Number(number);
        }
    }
    if raw.starts_with('[') || raw.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            return parsed;
        }
    }
    serde_json::Value::String(raw.to_string())
}

/// Set `value` at the dotted `key` path inside `target`, creating
/// intermediate objects as needed. Callers guarantee the root is an object.
fn set_dotted_path(target: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts.split_last().expect("keys are never empty");
    let mut current = target;
    for part in parents {
        let entry = current
            .as_object_mut()
            .expect("config root and intermediates are objects")
            .entry(*part)
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        current = entry;
    }
    current
        .as_object_mut()
        .expect("config root and intermediates are objects")
        .insert((*last).to_string(), value);
}

/// Which credential the `add`/`auth` flows should acquire for a config,
/// derived deterministically from the config's `auth.kind` tag — the same
/// tag the backends' auth-method DTOs use. OAuth is A4 (#205): the
/// interactive PKCE flow replaces the credential prompt.
enum CredentialKind {
    AppPassword,
    ApiToken,
    OAuth,
    None,
}

fn credential_kind_for(config: &serde_json::Value) -> CredentialKind {
    match config.pointer("/auth/kind").and_then(|v| v.as_str()) {
        Some("app_password") => CredentialKind::AppPassword,
        Some("api_token") => CredentialKind::ApiToken,
        Some("oauth") => CredentialKind::OAuth,
        _ => CredentialKind::None,
    }
}

/// Resolve the non-OAuth credential secret for `add` *before* the instance
/// exists: the matching flag wins, then an interactive `inquire` prompt. A
/// canceled prompt (Esc/Ctrl-D) exits, so the daemon never registers a
/// zombie `Setup` row for an aborted interactive run.
///
/// Returns `None` when the config declares no credential, or when stdin is
/// not a terminal and no flag was supplied — the caller proceeds with an
/// unauthenticated instance and warns (recoverable later via
/// `mimir connector auth <slug>`).
fn add_secret(
    config: &serde_json::Value,
    password: Option<String>,
    token: Option<String>,
) -> Option<String> {
    match credential_kind_for(config) {
        CredentialKind::AppPassword => {
            if token.is_some() {
                eprintln!(
                    "Warning: --token given but auth.kind is 'app_password' — ignoring it (pass --password instead)"
                );
            }
            password.or_else(|| prompt_secret("App password:"))
        }
        CredentialKind::ApiToken => {
            if password.is_some() {
                eprintln!(
                    "Warning: --password given but auth.kind is 'api_token' — ignoring it (pass --token instead)"
                );
            }
            token.or_else(|| prompt_secret("API token:"))
        }
        CredentialKind::OAuth => {
            if password.is_some() || token.is_some() {
                eprintln!(
                    "Warning: --password/--token given but auth.kind is 'oauth' — ignoring them (the PKCE flow obtains the token)"
                );
            }
            None
        }
        CredentialKind::None => {
            if password.is_some() || token.is_some() {
                eprintln!(
                    "Warning: --password/--token given but config declares no non-OAuth credential kind — ignoring them (set auth.kind=app_password or auth.kind=api_token)"
                );
            }
            None
        }
    }
}

/// Title-case a connector type for the default display name (`gmail` →
/// `Gmail`).
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Ask a yes/no question, defaulting to no. Non-terminal stdin (scripts,
/// pipes) aborts with a message pointing at `--yes` instead of hanging or
/// failing opaquely.
fn confirm(prompt: impl AsRef<str>) -> bool {
    if !std::io::stdin().is_terminal() {
        exit_with_error(
            "confirmation required — run in a terminal, or pass --yes to skip the prompt",
        );
    }
    match inquire::Confirm::new(prompt.as_ref())
        .with_default(false)
        .prompt()
    {
        Ok(confirmed) => confirmed,
        Err(e) => exit_with_error(format!("confirmation prompt failed: {e}")),
    }
}

/// Prompt for a secret value via `inquire`, unless stdin is not a terminal
/// (scripts/pipes), in which case no prompt is shown and `None` is returned
/// so the caller can warn instead.
fn prompt_secret(label: &str) -> Option<String> {
    if !std::io::stdin().is_terminal() {
        return None;
    }
    match inquire::Password::new(label)
        .without_confirmation()
        .prompt()
    {
        Ok(value) => Some(value),
        Err(e) => exit_with_error(format!("{label} prompt failed: {e}")),
    }
}

/// Coloured status text for the detail view (`active` green, `paused`
/// yellow, `setup` cyan, `error` red).
fn colored_status(status: &str) -> colored::ColoredString {
    match status {
        "active" => status.green(),
        "paused" => status.yellow(),
        "setup" => status.cyan(),
        "error" => status.red(),
        _ => status.normal(),
    }
}

/// Coloured auth-state text for the detail view (`authenticated` green,
/// `unauthenticated` yellow, `expired` red).
fn colored_auth(auth: &str) -> colored::ColoredString {
    match auth {
        "authenticated" => auth.green(),
        "unauthenticated" => auth.yellow(),
        "expired" => auth.red(),
        _ => auth.normal(),
    }
}
