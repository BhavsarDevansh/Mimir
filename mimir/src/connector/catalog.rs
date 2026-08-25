//! `mimir connector catalog` — discover the daemon's supported
//! `(connector_type, backend)` pairs (issue #271).

use mimir_api_types::ConnectorCatalogResponse;

use super::{exit_with_error, make_client, print_json, render_client_error};

/// List every `(connector_type, backend)` pair the daemon can construct as a
/// table (or raw JSON).
///
/// The daemon's registry is populated at startup from its cargo features, so
/// this is the authoritative discovery surface for `mimir connector add`:
/// users see exactly the backends that would be accepted.
pub async fn handle_connector_catalog(json: bool, transport: &crate::transport::DaemonTransport) {
    let client = make_client(transport);
    let resp = client
        .connector_catalog()
        .await
        .unwrap_or_else(|e| exit_with_error(render_client_error(e)));
    if json {
        print_json(&resp);
        return;
    }
    print_catalog_table(&resp);
}

/// Render the supported-pairs table. Cells stay plain (no ANSI) so column
/// widths measure correctly, matching the `connector list` table style.
fn print_catalog_table(resp: &ConnectorCatalogResponse) {
    use tabled::{Table, Tabled, settings::Style};

    if resp.entries.is_empty() {
        println!("No connector backends registered.");
        return;
    }

    #[derive(Tabled)]
    struct CatalogRow {
        connector_type: String,
        backend: String,
    }

    let rows: Vec<CatalogRow> = resp
        .entries
        .iter()
        .map(|entry| CatalogRow {
            connector_type: entry.connector_type.clone(),
            backend: entry.backend.clone(),
        })
        .collect();
    let mut table = Table::new(rows);
    table.with(Style::modern());
    println!("{table}");
}
