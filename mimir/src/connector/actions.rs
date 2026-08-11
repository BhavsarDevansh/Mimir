//! `mimir connector act` — write-back action dispatch.

use mimir_api_types::ConnectorActionRequest;

use super::{exit_with_error, make_client, print_json, render_client_error, resolve_connector};

/// Dispatch a write-back action (e.g. the Calendar connector's
/// `create_event` / `update_event` / `delete_event`), echoing the returned
/// `ActionResult` (`native_id` = resource href, `message` = e.g. the new
/// ETag). The payload is an inline JSON positional, a `--json-file`, or
/// `null` when omitted.
pub async fn handle_connector_act(
    slug: String,
    kind: String,
    payload: Option<String>,
    json_file: Option<std::path::PathBuf>,
    json: bool,
    base_url: &str,
) {
    let client = make_client(base_url);
    let conn = resolve_connector(&client, &slug).await;
    let payload = parse_action_payload(payload, json_file);

    let resp = client
        .connector_actions(conn.id, ConnectorActionRequest { kind, payload })
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));

    if json {
        print_json(&resp);
    }
    if !resp.success {
        exit_with_error(format!(
            "action failed: {}",
            resp.message.as_deref().unwrap_or("unknown error")
        ));
    }
    if !json {
        println!("Action succeeded.");
        if let Some(native_id) = &resp.native_id {
            println!("Native id: {native_id}");
        }
        if let Some(message) = &resp.message {
            println!("{message}");
        }
    }
}

/// Resolve the action payload from the inline JSON positional or the
/// `--json-file`, defaulting to `null` when neither is supplied. clap
/// enforces that the two sources are mutually exclusive.
fn parse_action_payload(
    payload: Option<String>,
    json_file: Option<std::path::PathBuf>,
) -> serde_json::Value {
    match (payload, json_file) {
        (Some(raw), None) => serde_json::from_str(&raw)
            .unwrap_or_else(|e| exit_with_error(format!("invalid action payload JSON: {e}"))),
        (None, Some(path)) => {
            let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                exit_with_error(format!("failed to read {}: {e}", path.display()))
            });
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                exit_with_error(format!(
                    "invalid action payload JSON in {}: {e}",
                    path.display()
                ))
            })
        }
        (None, None) => serde_json::Value::Null,
        (Some(_), Some(_)) => exit_with_error(
            "action payload cannot come from both a positional argument and --json-file",
        ),
    }
}
