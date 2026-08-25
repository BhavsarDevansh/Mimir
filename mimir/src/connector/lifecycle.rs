//! `mimir connector pause` / `resume` / `remove` / `forget` — lifecycle and
//! teardown subcommands.

use super::{
    confirm, exit_with_error, make_client, print_json, render_client_error, resolve_connector,
};

/// Pause a connector: stop its runner and flip its status to `Paused`.
pub async fn handle_connector_pause(
    slug: String,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let conn = resolve_connector(&client, &slug).await;
    let updated = client
        .connector_pause(conn.id)
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    if json {
        print_json(&updated);
        return;
    }
    println!("Connector '{slug}' paused (status: {}).", updated.status);
}

/// Resume a connector: re-spawn its runner and flip its status to `Active`.
pub async fn handle_connector_resume(
    slug: String,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let conn = resolve_connector(&client, &slug).await;
    let updated = client
        .connector_resume(conn.id)
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    if json {
        print_json(&updated);
        return;
    }
    println!("Connector '{slug}' resumed (status: {}).", updated.status);
}

/// Remove a connector: stop its runner, delete its credentials, and delete
/// the row, detaching provenance so ingested facts survive.
pub async fn handle_connector_remove(
    slug: String,
    yes: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let conn = resolve_connector(&client, &slug).await;
    if !yes
        && !confirm(format!(
            "Remove connector '{slug}'? Its credentials will be deleted and its provenance detached (ingested facts survive)."
        ))
    {
        println!("Aborted.");
        return;
    }
    client
        .connector_remove(conn.id)
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    println!("Connector '{slug}' removed.");
}

/// Forget a connector: trash every fact it sourced (recoverable from trash
/// for 30 days), then delete its credentials and row.
pub async fn handle_connector_forget(
    slug: String,
    yes: bool,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let conn = resolve_connector(&client, &slug).await;
    if !yes
        && !confirm(format!(
            "Forget connector '{slug}'? Every fact it sourced will be trashed (recoverable for 30 days) and its credentials and instance deleted."
        ))
    {
        println!("Aborted.");
        return;
    }
    let resp = client
        .connector_forget(conn.id)
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    if json {
        print_json(&resp);
        return;
    }
    println!(
        "Forgot connector '{slug}': {} fact(s) trashed.",
        resp.forgotten_count
    );
}
