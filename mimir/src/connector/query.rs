//! `mimir connector list` / `status` — instance overview and detail views.

use mimir_api_types::ConnectorResponse;

use super::{
    colored_auth, colored_status, exit_with_error, make_client, print_json, render_client_error,
    resolve_connector,
};

/// List every registered connector instance as a table (or raw JSON).
pub async fn handle_connector_list(json: bool, transport: &crate::transport::DaemonTransport) {
    let client = make_client(transport);
    let resp = client
        .connectors()
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    if json {
        print_json(&resp);
        return;
    }
    print_connector_table(&resp.connectors);
}

/// Show connector status: a detailed view for one slug, or the overview
/// table when no slug is given (matching the vision's "Checking Status"
/// UX).
pub async fn handle_connector_status(
    slug: Option<String>,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    match slug {
        Some(slug) => {
            let conn = resolve_connector(&client, &slug).await;
            if json {
                print_json(&conn);
                return;
            }
            print_connector_detail(&conn);
        }
        None => {
            let resp = client
                .connectors()
                .await
                .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
            if json {
                print_json(&resp);
                return;
            }
            print_connector_table(&resp.connectors);
        }
    }
}

/// Render the overview table. Cells stay plain (no ANSI) so column widths
/// measure correctly; colour is reserved for the detail view.
fn print_connector_table(connectors: &[ConnectorResponse]) {
    use tabled::{Table, Tabled, settings::Style};

    if connectors.is_empty() {
        println!("No connectors registered.");
        return;
    }

    #[derive(Tabled)]
    struct ConnectorRow {
        id: i32,
        connector_type: String,
        slug: String,
        backend: String,
        mode: String,
        status: String,
        auth: String,
        items: i64,
        last_sync: String,
    }

    let rows: Vec<ConnectorRow> = connectors
        .iter()
        .map(|c| ConnectorRow {
            id: c.id,
            connector_type: c.connector_type.clone(),
            slug: c.slug.clone(),
            backend: c.backend.clone(),
            mode: c.mode.clone().unwrap_or_else(|| "-".to_string()),
            status: c.status.clone(),
            auth: c.auth_state.clone(),
            items: c.item_count,
            last_sync: c.last_sync_at.as_deref().unwrap_or("-").to_string(),
        })
        .collect();
    let mut table = Table::new(rows);
    table.with(Style::modern());
    println!("{table}");
}

/// Render one instance with every field, colour-coded status/auth.
fn print_connector_detail(conn: &ConnectorResponse) {
    println!("ID:            {}", conn.id);
    println!("Type:          {}", conn.connector_type);
    println!("Slug:          {}", conn.slug);
    println!("Backend:       {}", conn.backend);
    println!("Display name:  {}", conn.display_name);
    println!("Mode:          {}", conn.mode.as_deref().unwrap_or("-"));
    println!("Status:        {}", colored_status(&conn.status));
    println!("Auth state:    {}", colored_auth(&conn.auth_state));
    println!("Items:         {}", conn.item_count);
    if let Some(cursor) = &conn.sync_cursor {
        println!("Sync cursor:   {cursor}");
    }
    if let Some(last_sync) = &conn.last_sync_at {
        println!("Last sync:     {last_sync}");
    }
    if let Some(last_error) = &conn.last_error {
        println!("Last error:    {last_error}");
    }
    println!("Created:       {}", conn.created_at);
    println!("Updated:       {}", conn.updated_at);
}
