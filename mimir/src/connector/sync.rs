//! `mimir connector sync` — manual sync trigger.

use mimir_api_types::SyncConnectorResponse;
use mimir_client::ClientError;

use super::{
    exit_with_error, is_connector_not_running, make_client, parse_duration, print_json,
    render_client_error, resolve_connector,
};

/// Trigger a manual sync cycle, echoing the daemon's outcome.
///
/// A 409 `CONNECTOR_NOT_RUNNING` response gets an actionable hint
/// (`mimir connector resume <slug>`) — a freshly added instance is `Setup`
/// and must be activated before it can sync.
pub async fn handle_connector_sync(
    slug: String,
    full: bool,
    since: Option<String>,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let conn = resolve_connector(&client, &slug).await;
    let since_secs = since.map(|raw| parse_duration(&raw).unwrap_or_else(|e| exit_with_error(e)));
    let request = mimir_api_types::SyncConnectorRequest {
        full,
        since: since_secs,
    };

    let outcome = client
        .connector_sync(conn.id, request)
        .await
        .unwrap_or_else(|e| match e {
            ClientError::Server { status: 409, message }
                if is_connector_not_running(&message) =>
            {
                exit_with_error(format!(
                    "connector '{slug}' is not running — activate it first with `mimir connector resume {slug}`"
                ))
            }
            other => exit_with_error(render_client_error(other)),
        });

    match outcome {
        SyncConnectorResponse::Ok {
            fetched,
            new_cursor,
        } => {
            if json {
                print_json(&SyncConnectorResponse::Ok {
                    fetched,
                    new_cursor,
                });
                return;
            }
            println!("Synced '{slug}': {fetched} item(s) fetched.");
            if let Some(cursor) = new_cursor {
                println!("New sync cursor: {cursor}");
            }
        }
        SyncConnectorResponse::AuthExpired { message } => {
            if json {
                print_json(&SyncConnectorResponse::AuthExpired {
                    message: message.clone(),
                });
            }
            exit_with_error(format!(
                "connector '{slug}' reported expired auth and has been paused by the daemon: {message}"
            ));
        }
        SyncConnectorResponse::Failed { message } => {
            if json {
                print_json(&SyncConnectorResponse::Failed {
                    message: message.clone(),
                });
            }
            exit_with_error(format!("sync of '{slug}' failed: {message}"));
        }
    }
}
